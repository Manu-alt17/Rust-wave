//! Captive-portal DNS hijack for the phone Wi-Fi provisioning hotspot.
//!
//! Answers every `A` query, for any hostname, with the AP's own IPv4
//! address, the same technique real captive portals (airports, cafes,
//! hotels, ...) and ESP-IDF's own `captive_portal` example use: whatever
//! hostname a phone tries to reach while joining -- its OS's automatic
//! connectivity probe, or the user manually opening any website -- resolves
//! to this device. Combined with the wildcard HTTP redirect in
//! `network_provision::espidf`, either path lands on the device's own
//! portal, so joining the hotspot QR code alone is enough to pop the
//! "Sign in to network" browser, or at least to make any manually opened
//! site redirect there -- no second QR code for the portal URL is needed.
//!
//! An earlier version of this only answered a fixed allowlist of known
//! connectivity-check hostnames (Android/Apple/Windows/Firefox/Samsung) and
//! refused everything else with NXDOMAIN, on the theory that answering
//! literally everything trips Android's DNS-hijack-detection heuristics
//! (which probe synthetic canary hostnames specifically to check for a
//! resolver that blindly resolves anything) and makes it distrust the
//! resolver. In practice that broke the one thing confirmed to work --
//! manually opening any site to reach the portal -- since a non-allowlisted
//! hostname a phone's browser actually tried now failed DNS resolution
//! outright, instead of reaching this server's HTTP redirect at all.
//! Answering every hostname trades that theoretical detection risk for a
//! fallback that reliably works.

/// Byte span `[12, end)` of `query`'s first question (QNAME + QTYPE +
/// QCLASS) plus its QTYPE, or `None` if `query` is too short to contain a
/// DNS header, declares zero questions, or has a truncated/malformed QNAME.
fn parse_question(query: &[u8]) -> Option<(&[u8], u16)> {
    if query.len() < 12 {
        return None;
    }
    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    if qdcount == 0 {
        return None;
    }

    // Walk the QNAME (a sequence of length-prefixed labels terminated by a
    // zero-length label) to find where QTYPE/QCLASS, and thus the question
    // section, ends.
    let mut pos = 12;
    loop {
        let label_len = *query.get(pos)? as usize;
        pos += 1;
        if label_len == 0 {
            break;
        }
        pos = pos.checked_add(label_len)?;
    }
    let question_end = pos.checked_add(4)?; // QTYPE (2) + QCLASS (2)
    let question = query.get(12..question_end)?;
    let qtype = u16::from_be_bytes([query[question_end - 4], query[question_end - 3]]);
    Some((question, qtype))
}

/// Build a DNS response for `query`: a wildcard `A` answer pointing at
/// `answer_ip` for any hostname the query asks an `A` record for, otherwise
/// (a `PTR`/`AAAA`/other query type, which a real answer at `answer_ip`
/// wouldn't be valid for) a real NXDOMAIN. Returns `None` only if `query` is
/// too malformed to answer at all (too short, zero questions, or a
/// truncated QNAME).
#[must_use]
pub fn build_response(query: &[u8], answer_ip: [u8; 4]) -> Option<Vec<u8>> {
    let (question, qtype) = parse_question(query)?;
    const QTYPE_A: u16 = 1;
    let resolve = qtype == QTYPE_A;

    let mut response = Vec::with_capacity(question.len() + 28);
    response.extend_from_slice(&query[0..2]); // ID, unchanged
    if resolve {
        // QR=1 (response), RD copied from the query, AA=1 (we are
        // authoritative for the wildcard answer we're about to make up).
        response.push(0x84 | (query[2] & 0x01));
        response.push(0x80); // RA=1, RCODE=0 (no error)
    } else {
        // Not authoritative for record types we're refusing; RCODE=3
        // (NXDOMAIN).
        response.push(0x80 | (query[2] & 0x01));
        response.push(0x83);
    }
    response.extend_from_slice(&[0, 1]); // QDCOUNT=1
    response.extend_from_slice(if resolve { &[0, 1] } else { &[0, 0] }); // ANCOUNT
    response.extend_from_slice(&[0, 0]); // NSCOUNT=0
    response.extend_from_slice(&[0, 0]); // ARCOUNT=0
    response.extend_from_slice(question);
    if resolve {
        response.extend_from_slice(&[0xC0, 0x0C]); // NAME: pointer to the question at offset 12
        response.extend_from_slice(&[0, 1]); // TYPE=A
        response.extend_from_slice(&[0, 1]); // CLASS=IN
        response.extend_from_slice(&[0, 0, 0, 60]); // TTL=60s
        response.extend_from_slice(&[0, 4]); // RDLENGTH=4
        response.extend_from_slice(&answer_ip);
    }
    Some(response)
}

/// Decode the QNAME of `query`'s first question into a dotted hostname, for
/// diagnostic logging only (`build_response` doesn't need this -- it just
/// echoes the question bytes back unparsed).
#[must_use]
pub fn query_name(query: &[u8]) -> Option<String> {
    if query.len() < 12 {
        return None;
    }
    let mut pos = 12;
    let mut name = String::new();
    loop {
        let label_len = *query.get(pos)? as usize;
        pos += 1;
        if label_len == 0 {
            break;
        }
        let label = query.get(pos..pos.checked_add(label_len)?)?;
        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(&String::from_utf8_lossy(label));
        pos += label_len;
    }
    Some(name)
}

#[cfg(target_os = "espidf")]
pub mod espidf {
    use std::{
        net::UdpSocket,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread::{self, JoinHandle},
        time::Duration,
    };

    use anyhow::{Context, Result};
    use log::{info, warn};

    const DNS_SERVER_STACK_BYTES: usize = 4096;
    const DNS_SOCKET_POLL_TIMEOUT: Duration = Duration::from_millis(500);

    /// RAII wrapper around the DNS hijack thread: constructing binds UDP:53
    /// and starts answering queries, dropping signals the thread to exit and
    /// joins it so the socket is freed before returning.
    pub struct CaptivePortalDns {
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl CaptivePortalDns {
        pub fn start(answer_ip: [u8; 4]) -> Result<Self> {
            let socket = UdpSocket::bind("0.0.0.0:53").context("bind captive-portal DNS socket")?;
            socket
                .set_read_timeout(Some(DNS_SOCKET_POLL_TIMEOUT))
                .context("set captive-portal DNS socket read timeout")?;

            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = Arc::clone(&stop);
            let handle = thread::Builder::new()
                .name("dns-portal".into())
                .stack_size(DNS_SERVER_STACK_BYTES)
                .spawn(move || run(&socket, answer_ip, &thread_stop))
                .context("spawn captive-portal DNS thread")?;

            info!("rustmix-wave=captive-portal-dns status=ready answer-ip={answer_ip:?}");
            Ok(Self {
                stop,
                handle: Some(handle),
            })
        }
    }

    impl Drop for CaptivePortalDns {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            info!("rustmix-wave=captive-portal-dns status=stopped");
        }
    }

    fn run(socket: &UdpSocket, answer_ip: [u8; 4], stop: &AtomicBool) {
        let mut buffer = [0_u8; 512];
        while !stop.load(Ordering::SeqCst) {
            let (len, source) = match socket.recv_from(&mut buffer) {
                Ok(received) => received,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(error) => {
                    warn!("rustmix-wave=captive-portal-dns status=recv-error error={error}");
                    continue;
                }
            };
            let name = super::query_name(&buffer[..len]).unwrap_or_default();
            match super::build_response(&buffer[..len], answer_ip) {
                Some(response) => {
                    if let Err(error) = socket.send_to(&response, source) {
                        warn!(
                            "rustmix-wave=captive-portal-dns status=send-error name={name} from={source} error={error}"
                        );
                    } else {
                        info!(
                            "rustmix-wave=captive-portal-dns status=answered name={name} from={source}"
                        );
                    }
                }
                None => warn!(
                    "rustmix-wave=captive-portal-dns status=unparseable-query name={name} from={source} bytes={len}"
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{build_response, query_name};

    /// A minimal single-question `A` query for `name`, as a phone's
    /// captive-portal probe would send.
    fn a_query(name: &str) -> Vec<u8> {
        let mut query = vec![
            0x12, 0x34, // ID
            0x01, 0x00, // flags: RD=1
            0x00, 0x01, // QDCOUNT=1
            0x00, 0x00, // ANCOUNT=0
            0x00, 0x00, // NSCOUNT=0
            0x00, 0x00, // ARCOUNT=0
        ];
        for label in name.split('.') {
            query.push(label.len() as u8);
            query.extend_from_slice(label.as_bytes());
        }
        query.push(0); // root label
        query.extend_from_slice(&[0, 1]); // QTYPE=A
        query.extend_from_slice(&[0, 1]); // QCLASS=IN
        query
    }

    #[test]
    fn any_hostname_gets_a_wildcard_answer() {
        let query = a_query("example.com");
        let response = build_response(&query, [192, 168, 71, 1]).unwrap();

        assert_eq!(&response[0..2], &query[0..2], "DNS transaction ID must be echoed");
        assert_eq!(response[2] & 0x80, 0x80, "QR bit must be set on a response");
        assert_eq!(response[3], 0x80, "RCODE must be 0 (NOERROR)");
        assert_eq!(&response[4..6], &[0, 1], "QDCOUNT");
        assert_eq!(&response[6..8], &[0, 1], "ANCOUNT");
        assert!(response.ends_with(&[192, 168, 71, 1]));
    }

    #[test]
    fn non_a_query_gets_nxdomain_instead_of_a_bogus_a_record() {
        let mut query = a_query("example.com");
        let qtype_offset = query.len() - 4;
        query[qtype_offset..qtype_offset + 2].copy_from_slice(&[0, 12]); // QTYPE=PTR
        let response = build_response(&query, [192, 168, 71, 1]).unwrap();

        assert_eq!(response[3] & 0x0F, 3, "RCODE must be 3 (NXDOMAIN)");
        assert_eq!(&response[6..8], &[0, 0], "ANCOUNT must be 0");
    }

    #[test]
    fn query_name_decodes_dotted_hostname_for_logging() {
        assert_eq!(
            query_name(&a_query("example.com")).as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn empty_or_truncated_queries_are_ignored() {
        assert!(build_response(&[], [10, 0, 0, 1]).is_none());
        assert!(build_response(&[0; 11], [10, 0, 0, 1]).is_none());
        let mut zero_questions = vec![0_u8; 12];
        zero_questions[5] = 0; // QDCOUNT=0
        assert!(build_response(&zero_questions, [10, 0, 0, 1]).is_none());
    }

    #[test]
    fn truncated_qname_does_not_panic() {
        let mut query = vec![0_u8; 12];
        query[5] = 1; // QDCOUNT=1
        query.push(200); // label length that runs past the end of the buffer
        assert!(build_response(&query, [10, 0, 0, 1]).is_none());
    }
}
