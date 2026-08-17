//! Canonical filename construction — the Rust authority.
//!
//! This mirrors `src/lib/naming/sanitize.ts`. Both exist deliberately:
//! Rust owns the filesystem (writing the vault, and later reconciling it by
//! parsing filenames back), while the TypeScript copy gives the review grid an
//! instant preview without an IPC round-trip on every keystroke.
//!
//! They cannot be allowed to drift, so both test suites are driven from the same
//! table: `tests/fixtures/naming-cases.json`.
//!
//! Lengths are counted in UTF-16 code units, because that is what Windows counts
//! against MAX_PATH — not bytes, and not chars.

use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// Windows MAX_PATH. Explorer never shipped the longPathAware manifest, so this
/// holds even where LongPathsEnabled=1. Verified 0 on the target machine.
pub const MAX_PATH: usize = 259;

pub const FOLDER_SLUG_MAX: usize = 48;
pub const TITLE_SLUG_MAX: usize = 60;

pub const UNKNOWN_DATE: &str = "0000-00-00";

/// UTF-16 length — the unit Windows measures paths in.
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Reserved DOS device names. Illegal as a whole path SEGMENT, folders included,
/// and still reserved with an extension appended ("CON.pdf").
fn is_reserved_device(s: &str) -> bool {
    let upper = s.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.chars().nth(3).is_some_and(|c| ('1'..='9').contains(&c)))
}

fn is_illegal(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '-')
        || c.is_whitespace()
        || (c as u32) < 0x20
}

/// Zero-width and bidi controls: invisible in Explorer, yet enough to make two
/// identical-looking folders distinct. Stripped from the slug, preserved in
/// display_name.
fn is_invisible(c: char) -> bool {
    matches!(c as u32,
        0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x2069 | 0xFEFF)
}

/// Truncate to at most `max` UTF-16 units without splitting a grapheme cluster.
pub fn truncate_graphemes(input: &str, max: usize) -> String {
    if utf16_len(input) <= max {
        return input.to_string();
    }
    let mut out = String::new();
    let mut len = 0usize;
    for g in input.graphemes(true) {
        let g_len = utf16_len(g);
        if len + g_len > max {
            break;
        }
        out.push_str(g);
        len += g_len;
    }
    out
}

/// Convert arbitrary user text into a path-segment-safe slug. Never returns an
/// empty string, a reserved device name, or a name Windows silently rewrites.
pub fn slugify(input: &str, max: usize, fallback: &str) -> String {
    let normalized: String = input.nfc().filter(|c| !is_invisible(*c)).collect();

    // Fold every illegal char and separator to '-', then collapse runs.
    let mut s = String::with_capacity(normalized.len());
    let mut last_dash = false;
    for c in normalized.chars() {
        if is_illegal(c) {
            if !last_dash {
                s.push('-');
                last_dash = true;
            }
        } else {
            s.push(c);
            last_dash = false;
        }
    }

    let trim = |s: &str| s.trim_matches(|c| c == '-' || c == '.').to_string();
    let mut s = trim(&s);

    s = truncate_graphemes(&s, max);
    // Truncation can re-expose a trailing separator or dot.
    s = s.trim_end_matches(|c| c == '-' || c == '.').to_string();

    if s.is_empty() {
        return fallback.to_string();
    }
    if is_reserved_device(&s) {
        return format!("{s}_");
    }
    s
}

pub fn patient_slug(display_name: &str) -> String {
    slugify(display_name, FOLDER_SLUG_MAX, "Unknown-Patient")
}

pub fn title_slug(title: &str) -> String {
    slugify(title, TITLE_SLUG_MAX, "Untitled")
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Accepts real dates plus two sentinels that still sort correctly:
/// 'YYYY-MM-00' (month known, day not) and '0000-00-00' (unknown).
pub fn is_valid_doc_date(d: &str) -> bool {
    let b = d.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !d
        .chars()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    if d == UNKNOWN_DATE {
        return true;
    }

    let (y, m, day) = match (d[0..4].parse::<i32>(), d[5..7].parse::<u32>(), d[8..10].parse::<u32>()) {
        (Ok(y), Ok(m), Ok(day)) => (y, m, day),
        _ => return false,
    };

    if !(1900..=2999).contains(&y) || !(1..=12).contains(&m) {
        return false;
    }
    if day == 0 {
        return true; // month-only sentinel
    }
    day <= days_in_month(y, m)
}

/// Undated documents are quarantined rather than filed into a wrong year.
pub fn year_folder(doc_date: &str) -> String {
    if !is_valid_doc_date(doc_date) || doc_date.starts_with("0000") {
        return "Undated".to_string();
    }
    doc_date[0..4].to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltName {
    pub file_name: String,
    pub patient_folder: String,
    pub year_folder: String,
    /// Vault-relative, backslash separated.
    pub rel_path: String,
    pub title_truncated: bool,
}

/// Build the canonical name and vault-relative path, shrinking the title as needed
/// to fit MAX_PATH. The date and patient are never sacrificed — they are what make
/// the file sortable and self-describing when lifted out of the tree on its own.
pub fn build_name(
    doc_date: &str,
    patient_name: &str,
    title: &str,
    ext: &str,
    seq: u32,
    vault_root_len: usize,
) -> BuiltName {
    let date = if is_valid_doc_date(doc_date) {
        doc_date.to_string()
    } else {
        UNKNOWN_DATE.to_string()
    };
    let p_slug = patient_slug(patient_name);
    let year = year_folder(&date);
    let ext = slugify(ext, 8, "bin").to_lowercase();

    let suffix = if seq > 1 {
        format!("__{seq:02}")
    } else {
        String::new()
    };

    // root + pSlug \ year \ date _ pSlug _ <title> suffix . ext
    let fixed = vault_root_len
        + utf16_len(&p_slug) + 1
        + year.len() + 1
        + date.len() + 1
        + utf16_len(&p_slug) + 1
        + suffix.len() + 1
        + ext.len();

    let budget = MAX_PATH.saturating_sub(fixed);
    let uncapped = slugify(title, usize::MAX, "Untitled");
    let wanted = title_slug(title);

    let t_slug = if budget < utf16_len(&wanted) {
        slugify(&wanted, budget.max(1), "x")
    } else {
        wanted
    };
    let title_truncated = t_slug != uncapped;

    let file_name = format!("{date}_{p_slug}_{t_slug}{suffix}.{ext}");
    BuiltName {
        rel_path: format!("{p_slug}\\{year}\\{file_name}"),
        file_name,
        patient_folder: p_slug,
        year_folder: year,
        title_truncated,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedName {
    pub doc_date: String,
    pub patient_slug: String,
    pub title: String,
    pub seq: u32,
    pub ext: String,
}

/// Read a canonical filename back into its parts.
///
/// This is the disaster-recovery story the naming scheme exists for: if the
/// database is lost, patient, year, date and title are all recoverable from the
/// filenames alone. It is also how the reconciler adopts a file the user dropped
/// straight into a vault folder.
pub fn parse_file_name(file_name: &str) -> Option<ParsedName> {
    let (stem, ext) = file_name.rsplit_once('.')?;
    if ext.is_empty() || ext.len() > 8 {
        return None;
    }

    // date _ patient _ title, where the title may itself contain underscores.
    let (date, rest) = stem.split_once('_')?;
    if !is_valid_doc_date(date) {
        return None;
    }
    let (patient, title_part) = rest.split_once('_')?;
    if patient.is_empty() || title_part.is_empty() {
        return None;
    }

    // A trailing __NN is a collision suffix, not part of the title.
    let (title, seq) = match title_part.rsplit_once("__") {
        Some((head, tail)) if !head.is_empty() && tail.len() == 2 && tail.chars().all(|c| c.is_ascii_digit()) => {
            (head.to_string(), tail.parse::<u32>().unwrap_or(1))
        }
        _ => (title_part.to_string(), 1),
    };

    Some(ParsedName {
        doc_date: date.to_string(),
        patient_slug: patient.to_string(),
        title,
        seq,
        ext: ext.to_lowercase(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT_LEN: usize = 49; // "C:\Users\mohibur\Documents\MedicineReportTracker\"

    #[test]
    fn a_canonical_name_round_trips_back_into_its_parts() {
        let built = build_name("2026-03-14", "Rahim Uddin", "Thyroid Profile", "pdf", 1, ROOT_LEN);
        let parsed = parse_file_name(&built.file_name).expect("should parse");
        assert_eq!(parsed.doc_date, "2026-03-14");
        assert_eq!(parsed.patient_slug, "Rahim-Uddin");
        assert_eq!(parsed.title, "Thyroid-Profile");
        assert_eq!(parsed.seq, 1);
        assert_eq!(parsed.ext, "pdf");
    }

    #[test]
    fn a_collision_suffix_is_not_mistaken_for_part_of_the_title() {
        let built = build_name("2026-03-14", "Rahim", "CBC", "jpg", 7, ROOT_LEN);
        let parsed = parse_file_name(&built.file_name).unwrap();
        assert_eq!(parsed.title, "CBC");
        assert_eq!(parsed.seq, 7);
    }

    #[test]
    fn titles_containing_underscores_survive_the_round_trip() {
        let parsed = parse_file_name("2026-03-14_Rahim_USG_Whole_Abdomen.jpg").unwrap();
        assert_eq!(parsed.title, "USG_Whole_Abdomen");
        assert_eq!(parsed.seq, 1);
    }

    #[test]
    fn the_undated_sentinel_parses() {
        let parsed = parse_file_name("0000-00-00_Rahim_Prescription.jpg").unwrap();
        assert_eq!(parsed.doc_date, UNKNOWN_DATE);
    }

    #[test]
    fn files_that_are_not_ours_are_rejected_rather_than_guessed_at() {
        assert!(parse_file_name("IMG_4821.jpg").is_none());
        assert!(parse_file_name("scan.pdf").is_none());
        assert!(parse_file_name("14-03-2026_Rahim_CBC.jpg").is_none(), "wrong date format");
        assert!(parse_file_name("2026-13-40_Rahim_CBC.jpg").is_none(), "impossible date");
        assert!(parse_file_name("2026-03-14_Rahim.jpg").is_none(), "no title");
        assert!(parse_file_name("noextension").is_none());
    }

    /// The shared table both this suite and `tests/sanitize.test.ts` run against,
    /// so the two implementations cannot drift apart.
    #[test]
    fn matches_the_shared_case_table() {
        let raw = include_str!("../../tests/fixtures/naming-cases.json");
        let cases: serde_json::Value = serde_json::from_str(raw).expect("valid JSON");

        for c in cases["slugs"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            let kind = c["kind"].as_str().unwrap();
            let want = c["want"].as_str().unwrap();
            let got = match kind {
                "patient" => patient_slug(input),
                "title" => title_slug(input),
                other => panic!("unknown slug kind {other}"),
            };
            assert_eq!(got, want, "slug {kind}({input:?})");
        }

        for c in cases["dates"].as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            let want = c["valid"].as_bool().unwrap();
            assert_eq!(is_valid_doc_date(input), want, "is_valid_doc_date({input:?})");
        }

        for c in cases["names"].as_array().unwrap() {
            let got = build_name(
                c["date"].as_str().unwrap(),
                c["patient"].as_str().unwrap(),
                c["title"].as_str().unwrap(),
                c["ext"].as_str().unwrap(),
                c["seq"].as_u64().unwrap_or(1) as u32,
                ROOT_LEN,
            );
            assert_eq!(
                got.file_name,
                c["fileName"].as_str().unwrap(),
                "build_name for {c:?}",
            );
        }
    }

    #[test]
    fn reserved_device_names_are_escaped_including_as_folders() {
        assert_eq!(patient_slug("CON"), "CON_");
        assert_eq!(patient_slug("nul"), "nul_");
        assert_eq!(patient_slug("COM1"), "COM1_");
        assert_eq!(patient_slug("Connor"), "Connor"); // not reserved once longer
    }

    #[test]
    fn nfc_and_nfd_forms_collapse_to_one_folder() {
        assert_eq!(patient_slug("Jos\u{e9}"), patient_slug("Jose\u{301}"));
    }

    #[test]
    fn invisible_characters_cannot_create_a_twin_folder() {
        assert_eq!(patient_slug("Rahim\u{200b}Uddin"), patient_slug("RahimUddin"));
        assert_eq!(patient_slug("Rahim\u{202e}Uddin"), patient_slug("RahimUddin"));
    }

    #[test]
    fn path_never_exceeds_max_path_even_with_hostile_input() {
        let b = build_name(
            "2026-03-14",
            &"A".repeat(200),
            &"Ultrasonogram of Whole Abdomen with Doppler and Contrast ".repeat(5),
            "pdf",
            1,
            ROOT_LEN,
        );
        assert!(b.title_truncated);
        assert!(
            ROOT_LEN + utf16_len(&b.rel_path) <= MAX_PATH,
            "path was {}, limit {MAX_PATH}",
            ROOT_LEN + utf16_len(&b.rel_path)
        );
    }

    #[test]
    fn a_deep_onedrive_root_still_fits() {
        let deep = "C:\\Users\\mohibur\\OneDrive - Some Long Organisation Name\\Documents\\MedicineReportTracker\\".len();
        let b = build_name(
            "2026-03-14",
            "Mohammad Rahim Uddin Chowdhury",
            "Ultrasonogram of Whole Abdomen with Doppler",
            "pdf",
            1,
            deep,
        );
        assert!(deep + utf16_len(&b.rel_path) <= MAX_PATH);
    }

    #[test]
    fn collision_suffixes_sort_numerically_within_a_day() {
        let mut names: Vec<String> = [3u32, 1, 12, 2]
            .iter()
            .map(|s| build_name("2026-03-14", "Rahim", "CBC", "jpg", *s, ROOT_LEN).file_name)
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "2026-03-14_Rahim_CBC.jpg",
                "2026-03-14_Rahim_CBC__02.jpg",
                "2026-03-14_Rahim_CBC__03.jpg",
                "2026-03-14_Rahim_CBC__12.jpg",
            ]
        );
    }

    #[test]
    fn undated_documents_are_quarantined() {
        assert_eq!(year_folder("2026-03-14"), "2026");
        assert_eq!(year_folder(UNKNOWN_DATE), "Undated");
        assert_eq!(year_folder("garbage"), "Undated");

        let b = build_name("nonsense", "Rahim", "CBC", "jpg", 1, ROOT_LEN);
        assert_eq!(b.year_folder, "Undated");
        assert!(b.file_name.starts_with(UNKNOWN_DATE));
    }
}
