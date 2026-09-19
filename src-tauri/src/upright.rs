//! Turning a staged image so its text reads upright.
//!
//! EXIF orientation covers exactly one case: a phone held sideways, whose camera
//! wrote a tag. A report lying sideways on the table with the phone held
//! straight, a page fed upside down through a flatbed, a screenshot, a WhatsApp
//! forward with the tag stripped — all arrive tagged upright with the text on
//! its side. No image library can tell; the only thing on the machine that can
//! read a page is the recognizer, so it is asked at every quarter turn and the
//! turn it reads best under wins.
//!
//! That costs three extra recognitions per image, roughly a second on a desktop.
//! It runs inside the same off-thread recognition step the review grid already
//! waits on, and the winning read is kept as the page's text, so an upright image
//! pays for three wasted passes and nothing else.
//!
//! The decision is made once. `text_rotation` on the staged row is NULL until
//! then, and after it is set — by this module or by the user turning the page
//! by hand — recognition never re-examines it. Otherwise a hand correction
//! would be undone the next time the queue was opened.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::imaging;
use crate::ocr::{self, OcrPage};

/// A quarter turn only. The value is clockwise degrees applied to the pixels.
pub const TURNS: [u16; 4] = [0, 90, 180, 270];

/// How much better another turn must read before the page is moved off the
/// orientation it arrived in. Sideways text produces some output — stray
/// letters, fragments read down a column — and this is what stops that noise
/// turning a page whose real text simply happens to be sparse.
const MARGIN_NUMERATOR: u32 = 3;
const MARGIN_DENOMINATOR: u32 = 2;

/// Below this the winning read is too thin to trust at all. A blank page or a
/// handwritten prescription scores near zero at every turn; it is left as it
/// came, which is the right answer for an image nothing can read.
const MIN_LEGIBLE: u32 = 24;

/// How much readable text a recognition produced.
///
/// Sideways text yields single characters and punctuation, which is easy.
/// Upside-down text is the hard case: the recognizer reads it as words, because
/// many glyphs turned over still look like glyphs — p and d, u and n, 6 and 9 —
/// and on the reference report it scored half of upright. What gives it away is
/// shape. Upright words are all lower case, ALL CAPS, or Capitalised, and
/// numbers are runs of digits; turned over they come out as `llnseU`, `Ise66ns`,
/// `9ZOZ/€0/9L`. So a word is split on punctuation and each piece counts only
/// when it is a run of letters with a sane case pattern, or a run of digits. A
/// dictionary would do better on English and nothing at all on the scripts
/// many reports are in; this needs no language.
pub fn legibility(page: &OcrPage) -> u32 {
    page.words
        .iter()
        .flat_map(|w| w.text.split(|c: char| !is_word_char(c)))
        .map(piece_score)
        .sum()
}

/// Letters, digits, and the marks that ride on letters — a Bengali vowel sign
/// or virama is not alphanumeric on its own, and splitting a word at each one
/// would leave a script that writes half its letters that way scoring nothing.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || unicode_normalization::char::is_combining_mark(c)
}

/// Letters or digits of one punctuation-free piece, or 0 if it is not the shape
/// of anything a page would print.
fn piece_score(piece: &str) -> u32 {
    let n = piece.chars().count() as u32;
    if n < 2 {
        return 0;
    }
    if piece.chars().all(|c| c.is_ascii_digit()) {
        return n;
    }
    if !piece.chars().all(|c| c.is_alphabetic() || unicode_normalization::char::is_combining_mark(c)) {
        return 0;
    }
    // A capital after the first letter, in a word that also has lower case, is
    // what a turned-over word looks like. Scripts without case pass through.
    let mut lower = false;
    let mut late_capital = false;
    for (i, c) in piece.chars().enumerate() {
        lower |= c.is_lowercase();
        late_capital |= i > 0 && c.is_uppercase();
    }
    if lower && late_capital {
        0
    } else {
        n
    }
}

/// Which turn to adopt, given the score at each of `TURNS` in order.
///
/// Pure, so the rule can be tested without a recognizer: keep the page as it
/// came unless some other turn is clearly better and clearly readable.
pub fn choose(scores: [u32; 4]) -> u16 {
    let as_is = scores[0];
    let (best_i, best) = scores
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| **s)
        .map(|(i, s)| (i, *s))
        .unwrap_or((0, 0));

    if best_i == 0 || best < MIN_LEGIBLE {
        return 0;
    }
    if best * MARGIN_DENOMINATOR > as_is * MARGIN_NUMERATOR {
        TURNS[best_i]
    } else {
        0
    }
}

pub struct Upright {
    /// The read at the turn that was adopted, in that turn's coordinates — which
    /// are the staged file's coordinates from now on.
    pub page: OcrPage,
    /// Clockwise degrees the staged file was turned by. 0 means untouched.
    pub rotation: u16,
}

/// A sibling of the staged file, named under the same ULID prefix so the
/// staging sweep on discard collects it if anything here is interrupted.
fn sibling(staged: &Path, suffix: &str) -> PathBuf {
    let stem = staged
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    staged.with_file_name(format!("{stem}.{suffix}.jpg"))
}

/// Recognise a staged image at every quarter turn, rewrite it at the turn that
/// reads best, and return that read.
///
/// On any failure after the first recognition the page is left as it came and
/// the as-is read is returned: a turn that cannot be written is not worth
/// failing recognition over.
pub fn recognize_upright(staged: &Path) -> Result<Upright, String> {
    let as_is = ocr::recognize(staged)?;

    let bytes = std::fs::read(staged).map_err(|e| format!("cannot read staged file: {e}"))?;
    let img = match image::load_from_memory(&bytes) {
        Ok(img) => img,
        // Not decodable here, though the recognizer managed: keep its read.
        Err(_) => return Ok(Upright { page: as_is, rotation: 0 }),
    };

    let mut reads: Vec<(u16, OcrPage)> = vec![(0, as_is)];
    let mut scratch = Vec::new();
    for degrees in TURNS.iter().copied().skip(1) {
        let path = sibling(staged, &format!("rot{degrees}"));
        let turned = imaging::rotate_cw(&img, degrees);
        let Ok(jpeg) = imaging::encode_print(&turned) else { continue };
        if std::fs::write(&path, jpeg).is_err() {
            continue;
        }
        scratch.push(path.clone());
        if let Ok(page) = ocr::recognize(&path) {
            reads.push((degrees, page));
        }
    }
    for path in scratch {
        let _ = std::fs::remove_file(path);
    }

    let mut scores = [0u32; 4];
    for (degrees, page) in &reads {
        let i = TURNS.iter().position(|t| t == degrees).unwrap_or(0);
        scores[i] = legibility(page);
    }
    let chosen = choose(scores);

    let winner = reads.iter().position(|(d, _)| *d == chosen).unwrap_or(0);
    let (_, page) = reads.swap_remove(winner);
    if chosen == 0 {
        return Ok(Upright { page, rotation: 0 });
    }

    match write_turned(staged, &bytes, chosen) {
        Ok(()) => Ok(Upright { page, rotation: chosen }),
        // The file could not be replaced, so the read at 0° is the one that
        // matches what is on disk. Recover it rather than return a read whose
        // word boxes belong to an image that does not exist.
        Err(_) => {
            let at_zero = reads.into_iter().find(|(d, _)| *d == 0).map(|(_, p)| p);
            Ok(Upright {
                page: at_zero.unwrap_or_default(),
                rotation: 0,
            })
        }
    }
}

/// Replace the staged print derivative and its thumbnail with turned copies.
fn write_turned(staged: &Path, bytes: &[u8], degrees: u16) -> Result<(), String> {
    let turned = imaging::turn_staged(bytes, degrees)?;
    std::fs::write(staged, &turned.print_jpeg)
        .map_err(|e| format!("cannot rewrite staged file: {e}"))?;
    let _ = std::fs::write(sibling(staged, "thumb"), &turned.thumb_jpeg);
    Ok(())
}

/// Turn a staged image by hand.
///
/// The detector's choice, or its refusal to choose, is overridden and never
/// revisited. The recognised text is dropped: it was read at an orientation the
/// user has just said was wrong, and a fresh read at this one — a plain read,
/// since the turn is now decided — costs a third of a second. Returns the total
/// turn now applied to the file, so the row can say so.
pub fn turn_by_hand(
    conn: &Connection,
    user_id: &str,
    ingest_id: &str,
    degrees: u16,
) -> Result<u16, String> {
    if !TURNS.contains(&(degrees % 360)) {
        return Err("A page can only be turned by a quarter turn.".into());
    }

    let (staged, kind, current, document_id): (Option<String>, String, Option<u16>, Option<String>) =
        conn.query_row(
            "SELECT staged_path, file_kind, text_rotation, document_id FROM ingest_items
              WHERE id = ?1 AND owner_user_id = ?2",
            params![ingest_id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| "No such file in the queue.".to_string())?;

    if document_id.is_some() {
        return Err("That file has already been filed.".into());
    }
    if kind == "pdf" {
        return Err("Only a photo or a scan can be turned here.".into());
    }
    let staged = PathBuf::from(staged.ok_or("That file is no longer staged.")?);

    let bytes = std::fs::read(&staged).map_err(|e| format!("cannot read the staged file: {e}"))?;
    write_turned(&staged, &bytes, degrees % 360)?;

    let total = (current.unwrap_or(0) + degrees) % 360;
    conn.execute(
        "UPDATE ingest_items
            SET text_rotation = ?2, ocr_text = NULL, ocr_json = NULL, ocr_at = NULL,
                updated_at = datetime('now')
          WHERE id = ?1",
        params![ingest_id, total],
    )
    .map_err(|e| format!("cannot record the turn: {e}"))?;

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::OcrWord;

    fn page_of(words: &[&str]) -> OcrPage {
        OcrPage {
            words: words
                .iter()
                .map(|w| OcrWord {
                    text: (*w).to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn legibility_counts_letters_in_words_and_ignores_fragments() {
        assert_eq!(legibility(&page_of(&["Haemoglobin"])), 11);
        assert_eq!(legibility(&page_of(&["14/03/2026"])), 8, "a date is digit runs");
        assert_eq!(legibility(&page_of(&["PDC-2026-88214"])), 3 + 4 + 5);
        assert_eq!(legibility(&page_of(&["TSH", "THYROID", "Dr."])), 3 + 7 + 2);
        assert_eq!(legibility(&page_of(&["l", "|", "—", "'", "1"])), 0, "single marks are noise");
        assert_eq!(legibility(&page_of(&["-.-.a.b"])), 0, "punctuation and singles");
        assert_eq!(legibility(&page_of(&["রক্ত"])), 4, "a script without case still counts");
        assert_eq!(legibility(&page_of(&[])), 0);
    }

    #[test]
    fn upside_down_words_do_not_count() {
        // Verbatim from the reference report recognised at 180°.
        assert_eq!(legibility(&page_of(&["9ZOZ/€0/9L"])), 0, "digits and letters mixed");
        assert_eq!(legibility(&page_of(&["llnseU"])), 0, "a capital after lower case");
        assert_eq!(legibility(&page_of(&["Ise66ns"])), 0);
        assert_eq!(legibility(&page_of(&["311dOUd"])), 0);
        // Some survive — nothing about shape says GIOUAHL is not a word — which is
        // why the margin exists.
        assert_eq!(legibility(&page_of(&["GIOUAHL"])), 7);
    }

    #[test]
    fn the_page_is_kept_as_it_came_unless_another_turn_is_clearly_better() {
        assert_eq!(choose([300, 20, 40, 15]), 0, "upright and readable");
        assert_eq!(choose([20, 300, 15, 40]), 90, "sideways");
        assert_eq!(choose([30, 10, 280, 12]), 180, "upside down");
        assert_eq!(choose([18, 12, 25, 310]), 270);
        assert_eq!(choose([0, 0, 0, 0]), 0, "blank page");
        assert_eq!(choose([5, 20, 3, 4]), 0, "too thin to trust at any turn");
        assert_eq!(choose([100, 130, 20, 20]), 0, "not clearly better: leave it");
        assert_eq!(choose([100, 151, 20, 20]), 90, "just over the margin");
        assert_eq!(choose([100, 150, 20, 20]), 0, "exactly on the margin stays put");
    }

    fn fixture() -> PathBuf {
        let f = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("tests")
            .join("fixtures")
            .join("lab-report-synthetic.jpg");
        assert!(f.exists(), "fixture missing: {}", f.display());
        f
    }

    /// The fixture, turned by `degrees` clockwise, staged under a ULID name.
    fn staged_turned(name: &str, degrees: u16) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("mrt-upright-{name}-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = std::fs::read(fixture()).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        let turned = imaging::rotate_cw(&img, degrees);
        let staged = dir.join(format!("{}.jpg", ulid::Ulid::new()));
        std::fs::write(&staged, imaging::encode_print(&turned).unwrap()).unwrap();
        (dir, staged)
    }

    #[cfg(windows)]
    #[test]
    fn scores_at_every_turn_of_a_real_report_separate_upright_from_the_rest() {
        // Calibration as much as a test: the margins above are only right if a
        // real page reads several times better upright than any other way.
        let mut scores = [0u32; 4];
        for (i, degrees) in TURNS.iter().enumerate() {
            let (dir, staged) = staged_turned("cal", *degrees);
            let page = ocr::recognize(&staged).unwrap();
            scores[i] = legibility(&page);
            // MRT_DUMP=1 prints what was read at each turn — how the shape
            // rules in `piece_score` were arrived at.
            if std::env::var("MRT_DUMP").is_ok() {
                eprintln!("--- {degrees}: {}", page.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "));
            }
            let _ = std::fs::remove_dir_all(dir);
        }
        eprintln!("legibility at 0/90/180/270 cw: {scores:?}");
        for i in 1..4 {
            assert!(
                scores[0] * MARGIN_DENOMINATOR > scores[i] * MARGIN_NUMERATOR,
                "upright ({}) must beat {}° ({}) by the margin",
                scores[0],
                TURNS[i],
                scores[i]
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_sideways_report_is_turned_back_and_its_date_read() {
        // Turned 90° clockwise on the way in, so 270° more brings it upright.
        let (dir, staged) = staged_turned("side", 90);
        let before = image::load_from_memory(&std::fs::read(&staged).unwrap()).unwrap();

        let up = recognize_upright(&staged).unwrap();
        assert_eq!(up.rotation, 270);
        assert!(up.page.text.contains("14/03/2026"), "got: {}", up.page.text);

        let after = image::load_from_memory(&std::fs::read(&staged).unwrap()).unwrap();
        assert_eq!((after.width(), after.height()), (before.height(), before.width()));
        assert!(sibling(&staged, "thumb").exists(), "thumbnail redrawn to match");
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().contains(".rot")),
            "scratch turns are cleaned up"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn an_upside_down_report_is_turned_back() {
        let (dir, staged) = staged_turned("flip", 180);
        let up = recognize_upright(&staged).unwrap();
        assert_eq!(up.rotation, 180);
        assert!(up.page.text.contains("Sample Collected"), "got: {}", up.page.text);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn an_upright_report_is_left_alone() {
        let (dir, staged) = staged_turned("up", 0);
        let before = std::fs::read(&staged).unwrap();
        let up = recognize_upright(&staged).unwrap();
        assert_eq!(up.rotation, 0);
        assert_eq!(std::fs::read(&staged).unwrap(), before, "file untouched");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_blank_page_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("mrt-upright-blank-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let staged = dir.join(format!("{}.jpg", ulid::Ulid::new()));
        let blank = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            800,
            600,
            image::Rgb([255, 255, 255]),
        ));
        std::fs::write(&staged, imaging::encode_print(&blank).unwrap()).unwrap();

        let up = recognize_upright(&staged).unwrap();
        assert_eq!(up.rotation, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn turning_by_hand_rewrites_the_file_and_forgets_the_old_read() {
        let (dir, staged) = staged_turned("hand", 0);
        let conn = crate::db::open(&dir.join("app.db")).unwrap();
        let user = crate::db::ensure_user(&conn).unwrap();
        let id = ulid::Ulid::new().to_string();
        conn.execute(
            "INSERT INTO ingest_items (id, owner_user_id, batch_id, src_path, staged_path, status,
                file_kind, byte_size, ocr_text, ocr_json, created_at, updated_at)
             VALUES (?1, ?2, 'b', 'x.jpg', ?3, 'needs_date', 'jpeg', 1, 'old', '[{}]',
                datetime('now'), datetime('now'))",
            params![id, user, staged.display().to_string()],
        )
        .unwrap();
        let before = image::load_from_memory(&std::fs::read(&staged).unwrap()).unwrap();

        assert_eq!(turn_by_hand(&conn, &user, &id, 90).unwrap(), 90);
        let sideways = image::load_from_memory(&std::fs::read(&staged).unwrap()).unwrap();
        assert_eq!((sideways.width(), sideways.height()), (before.height(), before.width()));
        assert!(sibling(&staged, "thumb").exists(), "thumbnail redrawn");

        assert_eq!(turn_by_hand(&conn, &user, &id, 90).unwrap(), 180, "turns accumulate");
        assert_eq!(turn_by_hand(&conn, &user, &id, 180).unwrap(), 0, "and wrap");
        assert!(turn_by_hand(&conn, &user, &id, 45).is_err(), "quarter turns only");

        let after = image::load_from_memory(&std::fs::read(&staged).unwrap()).unwrap();
        assert_eq!((after.width(), after.height()), (before.width(), before.height()));

        let (rotation, text, json): (Option<u16>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT text_rotation, ocr_text, ocr_json FROM ingest_items WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(rotation, Some(0), "decided, even when back where it started");
        assert!(text.is_none() && json.is_none(), "the old read is gone");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_pdf_and_an_unknown_row_cannot_be_turned() {
        let dir = std::env::temp_dir().join(format!("mrt-upright-refuse-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = crate::db::open(&dir.join("app.db")).unwrap();
        let user = crate::db::ensure_user(&conn).unwrap();
        conn.execute(
            "INSERT INTO ingest_items (id, owner_user_id, batch_id, src_path, staged_path, status,
                file_kind, byte_size, created_at, updated_at)
             VALUES ('pdf', ?1, 'b', 'x.pdf', 'x.pdf', 'needs_date', 'pdf', 1,
                datetime('now'), datetime('now'))",
            params![user],
        )
        .unwrap();
        assert!(turn_by_hand(&conn, &user, "pdf", 90).unwrap_err().contains("photo or a scan"));
        assert!(turn_by_hand(&conn, &user, "missing", 90).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
