//! Native offline Dictionary engine compatible with the Rustmix X4 SD pack,
//! behind the Reader's in-page word lookup.
//!
//! The X4 pack remains authoritative on removable storage:
//! `/sdcard/RUSTMIX/APPS/DICT/INDEX.TXT` selects one bounded
//! `DATA/*.JSN` prefix shard.
//!
//! INDEX.TXT is byte-sorted, so it is binary-searched in place on the SD card
//! instead of being loaded whole: a full Italian pack indexes ~28k shards
//! (~600 KB), and parsing that up front was the dominant cost of the first
//! lookup. Shards may also live one directory deeper (`DATA/CA/CASA.JSN`) so
//! FAT never has to scan a single ~28k-entry directory on every open.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{bail, Context, Result};

/// Rustmix X4-compatible dictionary app root.
pub const DICTIONARY_ROOT: &str = "/sdcard/RUSTMIX/APPS/DICT";
/// X4 pack shard index filename.
pub const DICTIONARY_INDEX_FILE: &str = "INDEX.TXT";
/// Prefix-shard files stay intentionally small for bounded SD reads.
pub const DICTIONARY_SHARD_MAX_BYTES: usize = 16 * 1024;
/// Prefix mode retains only a compact page of matches.
pub const DICTIONARY_MATCH_LIMIT: usize = 8;
/// Small reads per binary-search probe: index rows are ~20 bytes long.
const INDEX_PROBE_BUFFER_BYTES: usize = 256;
/// Upper bound on broader shards considered by one prefix search; results
/// stop at `DICTIONARY_MATCH_LIMIT` long before this in practice.
const PREFIX_SHARD_LIMIT: usize = 32;
/// "plurale di babbea" -> "femminile di babbeo" -> babbeo is the deepest
/// chain the Italian pack produces in practice.
const FORM_OF_MAX_HOPS: usize = 2;
/// Fits the reader definition panel (7 lines x 42 chars).
const EXPLAINED_DEFINITION_MAX_CHARS: usize = 290;
const DEFINITION_MAX_CHARS: usize = 220;
/// First-sense openings that mark an entry as a pure grammatical pointer
/// ("plurale di X", "terza persona singolare ... di Y") rather than a gloss.
const FORM_OF_HEADS: [&str; 22] = [
    "plurale",
    "femminile",
    "maschile",
    "participio",
    "gerundio",
    "infinito",
    "prima persona",
    "seconda persona",
    "terza persona",
    "imperativo",
    "congiuntivo",
    "condizionale",
    "variante",
    "variazione",
    "forma",
    "superlativo",
    "comparativo",
    "sinonimo",
    "diminutivo",
    "accrescitivo",
    "vezzeggiativo",
    "peggiorativo",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryIndexRow {
    pub name: String,
    pub relative_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryMatch {
    pub word: String,
    pub definition: String,
    pub shard: String,
    /// Whether `definition` already had its form-of pointer followed.
    pub explained: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryLookup {
    pub matches: Vec<DictionaryMatch>,
    pub prefix_mode: bool,
}

/// Handle on the on-SD INDEX.TXT. Holds no rows: every lookup binary-searches
/// the sorted file, so opening it costs one `stat` rather than a full read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryIndex {
    path: PathBuf,
}

impl DictionaryIndex {
    pub fn open(root: &Path) -> Result<Self> {
        let path = root.join(DICTIONARY_INDEX_FILE);
        let metadata = fs::metadata(&path)
            .with_context(|| format!("missing dictionary INDEX.TXT at {}", path.display()))?;
        if metadata.len() == 0 {
            bail!("empty dictionary INDEX.TXT");
        }
        Ok(Self { path })
    }

    fn cursor(&self) -> Result<IndexCursor> {
        let file = File::open(&self.path)
            .with_context(|| format!("missing dictionary INDEX.TXT at {}", self.path.display()))?;
        let len = file.metadata()?.len();
        Ok(IndexCursor {
            reader: BufReader::with_capacity(INDEX_PROBE_BUFFER_BYTES, file),
            len,
            line: Vec::new(),
        })
    }

    /// First usable row, used as a cheap "is the pack readable" probe.
    pub fn first_row(&self) -> Result<DictionaryIndexRow> {
        let mut cursor = self.cursor()?;
        cursor.reader.seek(SeekFrom::Start(0))?;
        cursor
            .next_row()?
            .ok_or_else(|| anyhow::anyhow!("empty dictionary INDEX.TXT"))
    }
}

struct IndexCursor {
    reader: BufReader<File>,
    len: u64,
    line: Vec<u8>,
}

impl IndexCursor {
    /// Positions the reader at the first line starting at or after
    /// `position` and returns that line's start, or `None` past the end.
    fn seek_line_start(&mut self, position: u64) -> Result<Option<u64>> {
        if position == 0 {
            self.reader.seek(SeekFrom::Start(0))?;
            return Ok((self.len > 0).then_some(0));
        }
        self.reader.seek(SeekFrom::Start(position - 1))?;
        self.line.clear();
        let skipped = self.reader.read_until(b'\n', &mut self.line)?;
        if self.line.last() != Some(&b'\n') {
            return Ok(None);
        }
        let start = position - 1 + skipped as u64;
        Ok((start < self.len).then_some(start))
    }

    /// Reads the raw line at the current position into `self.line`,
    /// returning its byte length including the newline (0 at EOF).
    fn read_raw_line(&mut self) -> Result<usize> {
        self.line.clear();
        Ok(self.reader.read_until(b'\n', &mut self.line)?)
    }

    /// Sort key of the line currently in `self.line`. Comments and blanks
    /// sort first, matching where packs put them (the top of the file).
    fn current_key(&self) -> String {
        let text = String::from_utf8_lossy(&self.line);
        let line = text.trim();
        if line.is_empty() || line.starts_with('#') {
            return String::new();
        }
        line.split('|')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase()
    }

    /// Positions the reader at the first row whose name is `>= target`.
    fn seek_lower_bound(&mut self, target: &str) -> Result<()> {
        let (mut low, mut high) = (0_u64, self.len);
        while low < high {
            let middle = low + (high - low) / 2;
            let Some(start) = self.seek_line_start(middle)? else {
                high = middle;
                continue;
            };
            let length = self.read_raw_line()?;
            if self.current_key().as_str() >= target {
                high = middle;
            } else {
                low = start + length.max(1) as u64;
            }
        }
        if self.seek_line_start(low)?.is_none() {
            self.reader.seek(SeekFrom::Start(self.len))?;
        }
        Ok(())
    }

    /// Next parsed row from the current position, skipping comments/blanks.
    fn next_row(&mut self) -> Result<Option<DictionaryIndexRow>> {
        loop {
            if self.read_raw_line()? == 0 {
                return Ok(None);
            }
            let text = String::from_utf8_lossy(&self.line).into_owned();
            if let Some(row) = parse_index_line(&text)? {
                return Ok(Some(row));
            }
        }
    }

    /// Rows for shard `name` plus its numbered overflow shards (`ABBAR`,
    /// `ABBAR2`, ...), which sort adjacently because digits precede letters.
    fn rows_named(&mut self, name: &str) -> Result<Vec<DictionaryIndexRow>> {
        self.seek_lower_bound(name)?;
        let mut rows = Vec::new();
        while let Some(row) = self.next_row()? {
            if shard_base(&row.name) != name {
                break;
            }
            rows.push(row);
        }
        Ok(rows)
    }

    /// Rows whose shard name starts with `prefix`, in index order.
    fn rows_with_prefix(&mut self, prefix: &str, limit: usize) -> Result<Vec<DictionaryIndexRow>> {
        self.seek_lower_bound(prefix)?;
        let mut rows = Vec::new();
        while rows.len() < limit {
            match self.next_row()? {
                Some(row) if row.name.starts_with(prefix) => rows.push(row),
                _ => break,
            }
        }
        Ok(rows)
    }
}

pub fn load_dictionary_index(root: &Path) -> Result<Vec<DictionaryIndexRow>> {
    let path = root.join(DICTIONARY_INDEX_FILE);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("missing dictionary INDEX.TXT at {}", path.display()))?;
    parse_dictionary_index(&text)
}

pub fn parse_dictionary_index(text: &str) -> Result<Vec<DictionaryIndexRow>> {
    let mut rows = Vec::new();
    for (line_number, raw_line) in text.lines().enumerate() {
        match parse_index_line(raw_line) {
            Ok(Some(row)) => rows.push(row),
            Ok(None) => {}
            Err(error) => bail!("bad dictionary index line {}: {error}", line_number + 1),
        }
    }
    if rows.is_empty() {
        bail!("empty dictionary INDEX.TXT");
    }
    Ok(rows)
}

/// One `NAME|DATA/[SUB/]NAME.JSN` row; `None` for comments and blanks.
fn parse_index_line(raw_line: &str) -> Result<Option<DictionaryIndexRow>> {
    let line = raw_line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let (name, relative_path) = line
        .split_once('|')
        .ok_or_else(|| anyhow::anyhow!("bad dictionary index row {line:?}"))?;
    let name = name.trim().to_ascii_uppercase();
    let relative_path = relative_path.trim().replace('\\', "/");
    if name.is_empty() || relative_path.is_empty() {
        bail!("bad dictionary index row {line:?}");
    }
    validate_relative_shard_path(&relative_path)?;
    Ok(Some(DictionaryIndexRow {
        name,
        relative_path,
    }))
}

/// Shards live at `DATA/NAME.JSN` (X4 layout) or one level deeper at
/// `DATA/SUB/NAME.JSN` (bucketed layout from relayout-dictionary-pack.sh).
fn validate_relative_shard_path(path: &str) -> Result<()> {
    if path.starts_with('/') || path.contains("..") || !path.ends_with(".JSN") {
        bail!("unsafe dictionary shard path {path:?}");
    }
    let segments: Vec<&str> = path.split('/').collect();
    let valid = match segments.as_slice() {
        ["DATA", file] => !file.is_empty(),
        ["DATA", sub, file] => {
            !file.is_empty()
                && !sub.is_empty()
                && sub.len() <= 8
                && sub
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        }
        _ => false,
    };
    if !valid {
        bail!("dictionary shard must live under DATA/: {path:?}");
    }
    Ok(())
}

pub fn lookup_dictionary(root: &Path, query: &str, prefix_mode: bool) -> Result<DictionaryLookup> {
    let query = normalize_query(query);
    if query.is_empty() {
        bail!("type a word first");
    }
    let index = DictionaryIndex::open(root)?;
    lookup_dictionary_with_index(root, &index, &query, prefix_mode)
}

/// One lookup's working set: an index cursor plus the shards already read,
/// so an exact miss falling back to prefix search, or a form-of pointer
/// landing in the same shard, never re-reads a shard from SD.
struct LookupSession<'a> {
    root: &'a Path,
    cursor: IndexCursor,
    shards: HashMap<String, String>,
}

impl<'a> LookupSession<'a> {
    fn new(root: &'a Path, index: &DictionaryIndex) -> Result<Self> {
        Ok(Self {
            root,
            cursor: index.cursor()?,
            shards: HashMap::new(),
        })
    }

    fn shard(&mut self, row: &DictionaryIndexRow) -> Result<&str> {
        if !self.shards.contains_key(&row.relative_path) {
            let text = read_bounded_shard(self.root, row)?;
            self.shards.insert(row.relative_path.clone(), text);
        }
        Ok(self.shards[&row.relative_path].as_str())
    }

    /// Exact match, trying the longest shard bucket first -- a shard can only
    /// contain a word if its bucket is a prefix of that word.
    fn exact(&mut self, query: &str) -> Result<Option<DictionaryMatch>> {
        for base in exact_shard_bases(query) {
            for row in self.cursor.rows_named(&base)? {
                let shard = self.shard(&row)?;
                if let Some((word, definition)) = extract_shard_matches(shard, query, false, 1)?
                    .into_iter()
                    .next()
                {
                    return Ok(Some(DictionaryMatch {
                        word,
                        definition,
                        shard: row.name.clone(),
                        explained: false,
                    }));
                }
            }
        }
        Ok(None)
    }

    fn prefix(&mut self, query: &str) -> Result<Vec<DictionaryMatch>> {
        let prefix = alpha_prefix(query);
        let mut rows = Vec::new();
        for base in exact_shard_bases(query) {
            rows.extend(self.cursor.rows_named(&base)?);
        }
        if prefix != "OTHERS" {
            for row in self.cursor.rows_with_prefix(&prefix, PREFIX_SHARD_LIMIT)? {
                if !rows.contains(&row) {
                    rows.push(row);
                }
            }
        }
        if rows.is_empty() {
            bail!("no shard in INDEX.TXT for {prefix}");
        }
        let mut matches: Vec<DictionaryMatch> = Vec::new();
        for row in &rows {
            let remaining = DICTIONARY_MATCH_LIMIT - matches.len();
            let shard = self.shard(row)?;
            for (word, definition) in extract_shard_matches(shard, query, true, remaining)? {
                if !matches.iter().any(|item| item.word == word) {
                    matches.push(DictionaryMatch {
                        word,
                        definition,
                        shard: row.name.clone(),
                        explained: false,
                    });
                }
            }
            if matches.len() >= DICTIONARY_MATCH_LIMIT {
                break;
            }
        }
        Ok(matches)
    }

    /// Follows "plurale di X" / "passato remoto di Y" pointers to the base
    /// word and appends its definition; returns `definition` unchanged when
    /// it is not a pointer or the base word is missing.
    fn explain(&mut self, word: &str, definition: &str) -> String {
        let mut seen = vec![normalize_query(word)];
        let mut current = definition.to_string();
        let mut intermediate: Vec<String> = Vec::new();
        let mut resolved: Option<(String, String)> = None;
        for _ in 0..FORM_OF_MAX_HOPS {
            let Some(lemma) = form_of_lemma(&current) else {
                break;
            };
            let key = normalize_query(&lemma);
            if seen.contains(&key) {
                break;
            }
            seen.push(key.clone());
            match self.exact(&key) {
                Ok(Some(entry)) => {
                    if let Some((_, previous)) = resolved.take() {
                        intermediate.push(first_sense(&previous).to_string());
                    }
                    current = entry.definition.clone();
                    resolved = Some((lemma.to_uppercase(), entry.definition));
                }
                _ => break,
            }
        }
        let Some((lemma, base_definition)) = resolved else {
            return definition.to_string();
        };
        let mut text = definition.to_string();
        if !intermediate.is_empty() {
            text.push_str(&format!(" ({})", intermediate.join("; ")));
        }
        text.push_str(&format!(" -> {lemma}: {base_definition}"));
        compact_text(&text, EXPLAINED_DEFINITION_MAX_CHARS)
    }
}

pub fn lookup_dictionary_with_index(
    root: &Path,
    index: &DictionaryIndex,
    query: &str,
    prefix_mode: bool,
) -> Result<DictionaryLookup> {
    let query = normalize_query(query);
    if query.is_empty() {
        bail!("type a word first");
    }
    let started = Instant::now();
    let mut session = LookupSession::new(root, index)?;
    if !prefix_mode {
        if let Some(entry) = session.exact(&query)? {
            log::debug!(
                "rustmix-wave=dictionary-lookup mode=exact hit=true elapsed-ms={}",
                started.elapsed().as_millis()
            );
            return Ok(DictionaryLookup {
                matches: vec![entry],
                prefix_mode: false,
            });
        }
    }
    let matches = session.prefix(&query)?;
    log::debug!(
        "rustmix-wave=dictionary-lookup mode=prefix matches={} shards-read={} elapsed-ms={}",
        matches.len(),
        session.shards.len(),
        started.elapsed().as_millis()
    );
    Ok(DictionaryLookup {
        matches,
        prefix_mode: true,
    })
}

/// Exact-only lookup with no prefix-search fallback. Only opens shards
/// whose bucket is itself a prefix of (or equal to) the query's own
/// alpha-prefix, so a query that reduces to a very short prefix -- e.g. an
/// elided Italian "L'IMPERTURBABILE" alpha-prefixing down to just "L" --
/// can't force a scan of every shard that happens to share that first
/// letter.
pub fn lookup_dictionary_exact(
    root: &Path,
    index: &DictionaryIndex,
    query: &str,
) -> Result<Option<DictionaryMatch>> {
    let query = normalize_query(query);
    if query.is_empty() {
        bail!("type a word first");
    }
    LookupSession::new(root, index)?.exact(&query)
}

/// [`lookup_dictionary_exact`] plus form-of resolution, sharing one index
/// cursor and shard cache between the word and its base word.
pub fn lookup_dictionary_explained(
    root: &Path,
    index: &DictionaryIndex,
    query: &str,
) -> Result<Option<DictionaryMatch>> {
    let query = normalize_query(query);
    if query.is_empty() {
        bail!("type a word first");
    }
    let started = Instant::now();
    let mut session = LookupSession::new(root, index)?;
    let mut entry = session.exact(&query)?;
    // Italian elisions ("DELL'ACQUA", "L'IMPERTURBABILE"): fall back to the
    // word after the apostrophe when the whole token is not an entry.
    if entry.is_none() {
        if let Some((_, tail)) = query.rsplit_once(['\'', '’']) {
            if tail.chars().count() >= 2 {
                entry = session.exact(tail)?;
            }
        }
    }
    let Some(mut entry) = entry else {
        log::debug!(
            "rustmix-wave=dictionary-lookup mode=explained hit=false elapsed-ms={}",
            started.elapsed().as_millis()
        );
        return Ok(None);
    };
    let found_ms = started.elapsed().as_millis();
    entry.definition = session.explain(&entry.word, &entry.definition);
    entry.explained = true;
    log::debug!(
        "rustmix-wave=dictionary-lookup mode=explained hit=true lookup-ms={found_ms} total-ms={} shards-read={}",
        started.elapsed().as_millis(),
        session.shards.len()
    );
    Ok(Some(entry))
}

/// Form-of resolution for a definition already in hand (e.g. the selected
/// prefix-search result). Falls back to `definition` on any error.
#[must_use]
pub fn resolve_definition(
    root: &Path,
    index: &DictionaryIndex,
    word: &str,
    definition: &str,
) -> String {
    match LookupSession::new(root, index) {
        Ok(mut session) => session.explain(word, definition),
        Err(_) => definition.to_string(),
    }
}

/// Shard buckets that could hold `query` exactly, longest first.
fn exact_shard_bases(query: &str) -> Vec<String> {
    let prefix = alpha_prefix(query);
    if prefix == "OTHERS" {
        return vec![prefix];
    }
    (1..=prefix.len())
        .rev()
        .map(|length| prefix[..length].to_string())
        .collect()
}

fn read_bounded_shard(root: &Path, row: &DictionaryIndexRow) -> Result<String> {
    let path = root.join(PathBuf::from(&row.relative_path));
    // One open per shard: every FAT open walks the containing directory.
    let mut file = File::open(&path)
        .with_context(|| format!("dictionary pack incomplete: missing {}", row.name))?;
    let length = file.metadata()?.len();
    if length > DICTIONARY_SHARD_MAX_BYTES as u64 {
        bail!("{} shard too large: {} bytes", row.name, length);
    }
    let mut text = String::with_capacity(length as usize);
    file.read_to_string(&mut text)
        .with_context(|| format!("read dictionary shard {}", path.display()))?;
    Ok(text)
}

fn shard_base(name: &str) -> &str {
    name.trim_end_matches(|character: char| character.is_ascii_digit())
}

#[must_use]
pub fn normalize_query(query: &str) -> String {
    query
        .trim()
        .trim_end_matches(|character| matches!(character, '*' | '_'))
        .trim()
        .to_ascii_uppercase()
}

#[must_use]
pub fn alpha_prefix(query: &str) -> String {
    let normalized = normalize_query(query);
    let mut output = String::new();
    for character in normalized.chars().take(5) {
        if character.is_ascii_alphabetic() {
            output.push(character);
        } else {
            break;
        }
    }
    if output.is_empty() && !normalized.is_empty() {
        "OTHERS".into()
    } else {
        output
    }
}

fn first_sense(definition: &str) -> &str {
    definition.split(" / ").next().unwrap_or(definition).trim()
}

/// Base word named by a pure form-of definition ("plurale di babbeo" ->
/// "babbeo"), judged on the first sense only. Glosses that merely end in
/// "di <word>" ("targa automobilistica ... di Bari") are not pointers.
#[must_use]
pub fn form_of_lemma(definition: &str) -> Option<String> {
    let sense = first_sense(definition)
        .trim_end_matches(|character: char| matches!(character, '.' | ';' | ',' | '…'))
        .trim();
    let lowered = sense.to_lowercase();
    // Whole-word heads only: "forma di" is a pointer, "formaggio di" is not.
    if !FORM_OF_HEADS.iter().any(|head| {
        lowered
            .strip_prefix(head)
            .is_some_and(|rest| rest.starts_with(' '))
    }) {
        return None;
    }
    let (_, lemma) = sense.rsplit_once(" di ")?;
    let lemma = lemma.trim();
    let is_word = !lemma.is_empty()
        && lemma
            .chars()
            .all(|character| character.is_alphabetic() || matches!(character, '\'' | '-'));
    is_word.then(|| lemma.to_string())
}

fn extract_shard_matches(
    json: &str,
    query: &str,
    prefix_mode: bool,
    limit: usize,
) -> Result<Vec<(String, String)>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let bytes = json.as_bytes();
    let mut position = skip_ws(bytes, 0);
    if bytes.get(position) != Some(&b'{') {
        bail!("parse failed: shard JSON");
    }
    position += 1;
    let mut matches = Vec::new();
    loop {
        position = skip_ws(bytes, position);
        match bytes.get(position) {
            Some(b'}') => break,
            Some(b'"') => {}
            _ => bail!("parse failed: shard key"),
        }
        let key_end = json_string_end(bytes, position)?;
        let key = &json[position + 1..key_end];
        position = skip_ws(bytes, key_end + 1);
        if bytes.get(position) != Some(&b':') {
            bail!("parse failed: shard separator");
        }
        position = skip_ws(bytes, position + 1);
        let value_end = json_value_end(bytes, position)?;
        if key == query || (prefix_mode && key.starts_with(query)) {
            matches.push((key.into(), compact_definition(&json[position..value_end])));
            if matches.len() >= limit {
                break;
            }
        }
        position = skip_ws(bytes, value_end);
        match bytes.get(position) {
            Some(b',') => position += 1,
            Some(b'}') => break,
            _ => bail!("parse failed: shard delimiter"),
        }
    }
    Ok(matches)
}

fn skip_ws(bytes: &[u8], mut position: usize) -> usize {
    while bytes
        .get(position)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    {
        position += 1;
    }
    position
}

fn json_string_end(bytes: &[u8], start: usize) -> Result<usize> {
    let mut escaped = false;
    for (offset, byte) in bytes.iter().enumerate().skip(start + 1) {
        if escaped {
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == b'"' {
            return Ok(offset);
        }
    }
    bail!("parse failed: unterminated string")
}

fn json_value_end(bytes: &[u8], start: usize) -> Result<usize> {
    let first = *bytes
        .get(start)
        .ok_or_else(|| anyhow::anyhow!("parse failed: missing value"))?;
    if first == b'"' {
        return Ok(json_string_end(bytes, start)? + 1);
    }
    if first != b'{' && first != b'[' {
        let mut position = start;
        while !matches!(
            bytes.get(position),
            None | Some(b',') | Some(b'}') | Some(b']')
        ) {
            position += 1;
        }
        return Ok(position);
    }
    let mut object_depth = 0usize;
    let mut array_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut position = start;
    while let Some(byte) = bytes.get(position) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
        } else {
            match *byte {
                b'"' => in_string = true,
                b'{' => object_depth += 1,
                b'}' => object_depth = object_depth.saturating_sub(1),
                b'[' => array_depth += 1,
                b']' => array_depth = array_depth.saturating_sub(1),
                _ => {}
            }
            if object_depth == 0 && array_depth == 0 {
                return Ok(position + 1);
            }
        }
        position += 1;
    }
    bail!("parse failed: unterminated value")
}

fn compact_definition(raw: &str) -> String {
    for field in ["def", "meaning", "definition", "text"] {
        if let Some(value) = extract_json_string_field(raw, field) {
            return compact_text(&dedupe_senses(&value), DEFINITION_MAX_CHARS);
        }
    }
    compact_text(raw, DEFINITION_MAX_CHARS)
}

/// Drops repeated " / "-separated senses ("femminile di cittadino /
/// femminile di cittadino") and swaps the pack's '…' for ASCII dots the
/// bitmap fonts can draw.
fn dedupe_senses(definition: &str) -> String {
    let mut senses: Vec<&str> = Vec::new();
    for sense in definition.split(" / ").map(str::trim) {
        if !sense.is_empty() && !senses.contains(&sense) {
            senses.push(sense);
        }
    }
    senses.join(" / ").replace('…', "...")
}

fn extract_json_string_field(raw: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let start = raw.find(&needle)? + needle.len();
    let rest = &raw[start..];
    let separator = rest.find(':')?;
    let rest = rest[separator + 1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let bytes = rest.as_bytes();
    let end = json_string_end(bytes, 0).ok()?;
    Some(unescape_json_string(&rest[1..end]))
}

fn unescape_json_string(raw: &str) -> String {
    raw.replace("\\\"", "\"")
        .replace("\\n", " ")
        .replace("\\r", " ")
        .replace("\\t", " ")
        .replace("\\\\", "\\")
}

fn compact_text(raw: &str, max_chars: usize) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let mut shortened: String = collapsed
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect();
    shortened.push_str("...");
    shortened
}

pub(crate) fn compact_error(message: &str) -> String {
    compact_text(message, 84)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        alpha_prefix, dedupe_senses, form_of_lemma, lookup_dictionary, lookup_dictionary_exact,
        lookup_dictionary_explained, normalize_query, parse_dictionary_index, DictionaryIndex,
        DICTIONARY_SHARD_MAX_BYTES,
    };

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustmix-wave-dict-{label}-{nanos}"))
    }

    fn write_pack(root: &std::path::Path) {
        fs::create_dir_all(root.join("DATA")).unwrap();
        fs::write(root.join("INDEX.TXT"), "AA|DATA/AA.JSN\nAB|DATA/AB.JSN\n").unwrap();
        fs::write(root.join("DATA/AA.JSN"), r#"{"AAM":[{"def":"Liquid measure","pos":""}],"AARD-VARK":[{"def":"African mammal","pos":""}]}"#).unwrap();
        fs::write(
            root.join("DATA/AB.JSN"),
            r#"{"AB":[{"def":"Month name","pos":""}],"AB-":[{"def":"Latin prefix","pos":""}]}"#,
        )
        .unwrap();
    }

    fn write_shard(root: &Path, relative: &str, json: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, json).unwrap();
    }

    /// Italian-style pack in the bucketed `DATA/SUB/NAME.JSN` layout, with
    /// numbered overflow shards and form-of chains.
    fn write_italian_pack(root: &Path) {
        fs::write(
            root.join("INDEX.TXT"),
            "# italian pack\n\
             A|DATA/A/A.JSN\n\
             AND|DATA/AN/AND.JSN\n\
             ANDAI|DATA/AN/ANDAI.JSN\n\
             ANDAR|DATA/AN/ANDAR.JSN\n\
             BABBE|DATA/BA/BABBE.JSN\n\
             BABBE2|DATA/BA/BABBE2.JSN\n\
             BABBU|DATA/BA/BABBU.JSN\n\
             CITT|DATA/CI/CITT.JSN\n\
             CITTA|DATA/CI/CITTA.JSN\n\
             ZZZ|DATA/ZZ/ZZZ.JSN\n",
        )
        .unwrap();
        write_shard(root, "DATA/A/A.JSN", r#"{"A":[{"def":"preposizione"}]}"#);
        write_shard(root, "DATA/AN/AND.JSN", r#"{"AND":[{"def":"sigla"}]}"#);
        write_shard(
            root,
            "DATA/AN/ANDAI.JSN",
            r#"{"ANDAI":[{"def":"prima persona singolare del passato remoto indicativo di andare"}]}"#,
        );
        write_shard(
            root,
            "DATA/AN/ANDAR.JSN",
            r#"{"ANDARE":[{"def":"muoversi, spostarsi a piedi"}]}"#,
        );
        write_shard(
            root,
            "DATA/BA/BABBE.JSN",
            r#"{"BABBEA":[{"def":"femminile di babbeo / femminile di babbeo"}],"BABBEE":[{"def":"plurale di babbea"}]}"#,
        );
        write_shard(
            root,
            "DATA/BA/BABBE2.JSN",
            r#"{"BABBEO":[{"def":"persona sciocca e credulona"}],"BABBEX":[{"def":"plurale di babbex"}]}"#,
        );
        write_shard(
            root,
            "DATA/BA/BABBU.JSN",
            r#"{"BABBUINO":[{"def":"scimmia"}]}"#,
        );
        write_shard(
            root,
            "DATA/CI/CITT.JSN",
            r#"{"CITTà":[{"def":"centro abitato…"}]}"#,
        );
        write_shard(
            root,
            "DATA/CI/CITTA.JSN",
            r#"{"CITTADINE":[{"def":"femminile plurale di cittadino"}],"CITTADINO":[{"def":"relativo alla città"}],"CITTADINI":[{"def":"plurale di città"}],"CITTADONE":[{"def":"sinonimo di nessuno"}]}"#,
        );
        write_shard(root, "DATA/ZZ/ZZZ.JSN", r#"{"ZZZ":[{"def":"sonno"}]}"#);
    }

    #[test]
    fn parses_x4_index_and_rejects_unsafe_paths() {
        let rows = parse_dictionary_index("# pack\nAA|DATA/AA.JSN\nAB|DATA/AB.JSN\n").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(parse_dictionary_index("CA|DATA/CA/CASA.JSN\n").is_ok());
        assert!(parse_dictionary_index("AA|../AA.JSN\n").is_err());
        assert!(parse_dictionary_index("AA|AA.JSN\n").is_err());
        assert!(parse_dictionary_index("AA|DATA/A/B/AA.JSN\n").is_err());
        assert!(parse_dictionary_index("AA|DATA/A.B/AA.JSN\n").is_err());
        assert!(parse_dictionary_index("AA|OTHER/AA/AA.JSN\n").is_err());
    }

    #[test]
    fn normalizes_queries_and_selects_alpha_prefix() {
        assert_eq!(normalize_query("  aard*  "), "AARD");
        assert_eq!(alpha_prefix("aard-vark"), "AARD");
        assert_eq!(alpha_prefix("123"), "OTHERS");
    }

    #[test]
    fn exact_lookup_and_prefix_fallback_use_bounded_x4_shards() {
        let root = temp_root("lookup");
        write_pack(&root);
        let exact = lookup_dictionary(&root, "AB", false).unwrap();
        assert!(!exact.prefix_mode);
        assert_eq!(exact.matches[0].definition, "Month name");
        let fallback = lookup_dictionary(&root, "AAR", false).unwrap();
        assert!(fallback.prefix_mode);
        assert_eq!(fallback.matches[0].word, "AARD-VARK");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_search_finds_first_last_overflow_and_missing_shards() {
        let root = temp_root("bsearch");
        fs::create_dir_all(&root).unwrap();
        write_italian_pack(&root);
        let index = DictionaryIndex::open(&root).unwrap();
        let exact = |word: &str| {
            lookup_dictionary_exact(&root, &index, word)
                .unwrap()
                .map(|entry| entry.definition)
        };
        assert_eq!(exact("A").as_deref(), Some("preposizione"));
        assert_eq!(exact("ZZZ").as_deref(), Some("sonno"));
        // BABBEO lives in the numbered overflow shard BABBE2.
        assert_eq!(
            exact("BABBEO").as_deref(),
            Some("persona sciocca e credulona")
        );
        assert_eq!(exact("CITTà").as_deref(), Some("centro abitato..."));
        assert_eq!(exact("QUASI"), None);
        assert_eq!(exact("ZZZZ"), None);
        let prefix = lookup_dictionary(&root, "BABB", true).unwrap();
        let words: Vec<_> = prefix.matches.iter().map(|m| m.word.as_str()).collect();
        assert!(words.contains(&"BABBEO") && words.contains(&"BABBUINO"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_only_pure_form_of_definitions() {
        assert_eq!(
            form_of_lemma("plurale di babbea").as_deref(),
            Some("babbea")
        );
        assert_eq!(
            form_of_lemma("prima persona singolare del passato remoto indicativo di andare")
                .as_deref(),
            Some("andare")
        );
        assert_eq!(
            form_of_lemma("plurale di cittadino / che sono abitanti di una città").as_deref(),
            Some("cittadino")
        );
        assert_eq!(
            form_of_lemma("sinonimo di bacheròzzolo").as_deref(),
            Some("bacheròzzolo")
        );
        assert_eq!(
            form_of_lemma("targa automobilistica e sigla utilizzata in ambito burocratico di Bari"),
            None
        );
        assert_eq!(form_of_lemma("nome proprio di persona maschile"), None);
        assert_eq!(form_of_lemma("formaggio tipico di Parma"), None);
        assert_eq!(form_of_lemma("plurale di un nome composto"), None);
    }

    #[test]
    fn dedupes_repeated_senses() {
        assert_eq!(
            dedupe_senses("femminile di cittadino / femminile di cittadino"),
            "femminile di cittadino"
        );
        assert_eq!(dedupe_senses("uno / due / uno"), "uno / due");
    }

    #[test]
    fn explained_lookup_follows_form_of_chains() {
        let root = temp_root("explain");
        fs::create_dir_all(&root).unwrap();
        write_italian_pack(&root);
        let index = DictionaryIndex::open(&root).unwrap();
        let explained = |word: &str| {
            lookup_dictionary_explained(&root, &index, word)
                .unwrap()
                .unwrap()
                .definition
        };
        assert_eq!(
            explained("ANDAI"),
            "prima persona singolare del passato remoto indicativo di andare -> ANDARE: muoversi, spostarsi a piedi"
        );
        assert_eq!(
            explained("CITTADINE"),
            "femminile plurale di cittadino -> CITTADINO: relativo alla città"
        );
        assert_eq!(
            explained("BABBEE"),
            "plurale di babbea (femminile di babbeo) -> BABBEO: persona sciocca e credulona"
        );
        assert_eq!(
            explained("CITTADINI"),
            "plurale di città -> CITTÀ: centro abitato..."
        );
        // Self-reference and a missing base word leave the text untouched.
        assert_eq!(explained("BABBEX"), "plurale di babbex");
        assert_eq!(explained("CITTADONE"), "sinonimo di nessuno");
        assert_eq!(explained("ANDARE"), "muoversi, spostarsi a piedi");
        // Elided article/preposition falls back to the word itself.
        assert_eq!(explained("dell'andare"), "muoversi, spostarsi a piedi");
        assert_eq!(explained("l’andare"), "muoversi, spostarsi a piedi");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_oversized_shard_before_reading() {
        let root = temp_root("oversized");
        fs::create_dir_all(root.join("DATA")).unwrap();
        fs::write(root.join("INDEX.TXT"), "AA|DATA/AA.JSN\n").unwrap();
        fs::write(
            root.join("DATA/AA.JSN"),
            vec![b' '; DICTIONARY_SHARD_MAX_BYTES + 1],
        )
        .unwrap();
        assert!(lookup_dictionary(&root, "AA", false)
            .unwrap_err()
            .to_string()
            .contains("shard too large"));
        fs::remove_dir_all(root).unwrap();
    }

    /// Smoke check against a real pack on disk:
    /// `RUSTMIX_DICT_PACK=/path/to/DICT cargo test -- --ignored real_pack`.
    #[test]
    #[ignore = "needs RUSTMIX_DICT_PACK pointing at a full dictionary pack"]
    fn real_pack_smoke() {
        let root = std::path::PathBuf::from(std::env::var("RUSTMIX_DICT_PACK").unwrap());
        let index = DictionaryIndex::open(&root).unwrap();
        for word in [
            "CASA",
            "ANDAI",
            "CITTADINE",
            "BABBEE",
            "CITTà",
            "L'IMPERTURBABILE",
        ] {
            let started = std::time::Instant::now();
            let entry = lookup_dictionary_explained(&root, &index, word).unwrap();
            println!(
                "{word} ({} ms): {:?}",
                started.elapsed().as_millis(),
                entry.map(|entry| entry.definition)
            );
        }
    }
}
