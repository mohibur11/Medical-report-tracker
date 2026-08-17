//! Building the merged PDF — the thing the whole app exists to produce.
//!
//! A doctor gets one file and scrolls it top to bottom. Everything here serves
//! that: pages are normalized to A4 portrait so the viewer never jumps or
//! rezooms between records, every page carries a header band saying whose report
//! it is and when, and the output is kept small enough to actually send.
//!
//! Merging is the easy part. The work is orientation (handled at ingest), page
//! size, output size, and the header band.

use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{params_from_iter, Connection};
use serde::{Deserialize, Serialize};

/// Gmail and Outlook both cap attachments at 25 MB. Anything above this has to
/// split, or it silently fails to send.
pub const EMAIL_LIMIT_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Preset {
    /// Vault files untouched. For printing.
    Original,
    /// What ingest already produced (~200 DPI, q78). WhatsApp default.
    Standard,
    /// Grayscale ~150 DPI. Printed lab reports stay perfectly legible and a long
    /// history fits under the email cap.
    EmailSafe,
}

impl Preset {
    fn long_edge(self) -> Option<u32> {
        match self {
            Preset::Original => None,
            Preset::Standard => Some(2000),
            Preset::EmailSafe => Some(1240), // ~150 DPI across A4
        }
    }
    fn quality(self) -> u8 {
        match self {
            Preset::Original => 92,
            Preset::Standard => 78,
            Preset::EmailSafe => 65,
        }
    }
    fn grayscale(self) -> bool {
        self == Preset::EmailSafe
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// Empty means every patient.
    pub patient_ids: Vec<String>,
    /// Empty means every year. Values are four-digit strings, or "Undated".
    pub years: Vec<String>,
    pub category_ids: Vec<String>,
    pub doc_types: Vec<String>,
    pub preset: Preset,
    /// Split into parts when a single file would exceed this. None disables it.
    pub max_bytes: Option<u64>,
    pub out_dir: String,
    pub base_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPart {
    pub path: String,
    pub bytes: u64,
    pub pages: u32,
    pub documents: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub parts: Vec<ExportPart>,
    pub total_documents: u32,
    /// Documents with no known date. They still export, at the end, but the user
    /// is told — a silently mis-ordered history is the failure this app must avoid.
    pub undated: u32,
    pub missing: Vec<String>,
}

struct Doc {
    id: String,
    doc_date: String,
    title: String,
    doc_type: String,
    rel_path: String,
    file_kind: String,
    patient: String,
}

/// Locate the bundled pdfcpu. Tauri copies `externalBin` next to the executable,
/// both in dev builds and in the installed app.
fn pdfcpu_path() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("PDFCPU_PATH") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate own exe: {e}"))?;
    let dir = exe.parent().ok_or("executable has no parent directory")?;

    for name in ["pdfcpu.exe", "pdfcpu-x86_64-pc-windows-msvc.exe"] {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("pdfcpu sidecar not found next to the application".into())
}

fn run_pdfcpu(exe: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(exe)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run pdfcpu: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pdfcpu {}: {}",
            args.first().copied().unwrap_or("?"),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn select_documents(conn: &Connection, user_id: &str, req: &ExportRequest) -> Result<Vec<Doc>, String> {
    let mut sql = String::from(
        "SELECT d.id, d.doc_date, d.title, d.doc_type, d.rel_path, d.file_kind, p.display_name
         FROM documents d
         JOIN patients p ON p.id = d.patient_id
         WHERE d.owner_user_id = ? AND d.trashed_at IS NULL AND d.missing_at IS NULL",
    );
    let mut binds: Vec<String> = vec![user_id.to_string()];

    let mut add_in = |column: &str, values: &[String], sql: &mut String, binds: &mut Vec<String>| {
        if values.is_empty() {
            return;
        }
        let holes = vec!["?"; values.len()].join(",");
        sql.push_str(&format!(" AND {column} IN ({holes})"));
        binds.extend(values.iter().cloned());
    };

    add_in("d.patient_id", &req.patient_ids, &mut sql, &mut binds);
    add_in("d.doc_type", &req.doc_types, &mut sql, &mut binds);

    if !req.years.is_empty() {
        // The year lives in the date itself; 'Undated' is the 0000 sentinel.
        let holes = vec!["?"; req.years.len()].join(",");
        sql.push_str(&format!(
            " AND (CASE WHEN d.doc_date LIKE '0000%' THEN 'Undated' ELSE substr(d.doc_date,1,4) END) IN ({holes})"
        ));
        binds.extend(req.years.iter().cloned());
    }

    if !req.category_ids.is_empty() {
        let holes = vec!["?"; req.category_ids.len()].join(",");
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM document_category dc
                          WHERE dc.document_id = d.id AND dc.category_id IN ({holes}))"
        ));
        binds.extend(req.category_ids.iter().cloned());
    }

    // Undated documents sort to the front as '0000-...', which would put unknown
    // material before a decade of history. Push them to the end instead.
    sql.push_str(
        " ORDER BY CASE WHEN d.doc_date LIKE '0000%' THEN 1 ELSE 0 END, d.doc_date, d.title",
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| format!("{e}\n{sql}"))?;
    let rows = stmt
        .query_map(params_from_iter(binds.iter()), |r| {
            Ok(Doc {
                id: r.get(0)?,
                doc_date: r.get(1)?,
                title: r.get(2)?,
                doc_type: r.get(3)?,
                rel_path: r.get(4)?,
                file_kind: r.get(5)?,
                patient: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Human-facing date. The UI and every generated page use DD/MM/YYYY; only disk
/// and database are ISO.
fn display_date(iso: &str) -> String {
    if iso.starts_with("0000") {
        return "date unknown".into();
    }
    match (iso.get(0..4), iso.get(5..7), iso.get(8..10)) {
        (Some(y), Some(m), Some("00")) => format!("{m}/{y}"),
        (Some(y), Some(m), Some(d)) => format!("{d}/{m}/{y}"),
        _ => iso.to_string(),
    }
}

/// Turn one vault file into a single A4 PDF carrying its header band.
fn prepare(
    exe: &Path,
    vault_root: &Path,
    work: &Path,
    doc: &Doc,
    index: usize,
    preset: Preset,
) -> Result<PathBuf, String> {
    let source = vault_root.join(doc.rel_path.replace('\\', "/"));
    if !source.exists() {
        return Err(format!("missing from the vault: {}", doc.rel_path));
    }

    let stem = format!("{index:04}");
    let as_pdf = work.join(format!("{stem}.pdf"));

    if doc.file_kind == "pdf" {
        // Never re-encode someone else's PDF: it loses fidelity for no benefit.
        std::fs::copy(&source, &as_pdf).map_err(|e| format!("cannot stage pdf: {e}"))?;
    } else {
        let image_path = match preset.long_edge() {
            None => source.clone(),
            Some(edge) => {
                let recoded = work.join(format!("{stem}.jpg"));
                recode(&source, &recoded, edge, preset.quality(), preset.grayscale())?;
                recoded
            }
        };
        // 'f:A4' fits the image to the page and 'pos:c' centres it — this is what
        // stops a continuous-scroll viewer jumping between records.
        run_pdfcpu(
            exe,
            &[
                "import",
                "f:A4, pos:c, sc:1.0 rel",
                &as_pdf.to_string_lossy(),
                &image_path.to_string_lossy(),
            ],
        )?;
    }

    // A4 portrait for everything, including source PDFs that arrive at Letter or
    // Folio/F4 — the latter is common in South Asian scans.
    let sized = work.join(format!("{stem}-a4.pdf"));
    run_pdfcpu(
        exe,
        &["resize", "formsize:A4P", &as_pdf.to_string_lossy(), &sized.to_string_lossy()],
    )?;

    // The header band is what actually delivers the "scroll top to bottom"
    // experience. Bookmarks are a bonus: WhatsApp's preview and several Android
    // viewers ignore outlines entirely, but every viewer renders the page.
    let header = format!(
        "{}   |   {}   |   {}",
        display_date(&doc.doc_date),
        doc.patient,
        doc.title
    );
    let stamped = work.join(format!("{stem}-final.pdf"));
    run_pdfcpu(
        exe,
        &[
            "stamp",
            "add",
            "--mode",
            "text",
            &header,
            // `rot` and `dia` are mutually exclusive in pdfcpu; setting rot:0 is
            // what turns off the default diagonal watermark placement.
            "font:Helvetica, points:8, pos:tc, off:0 -14, rot:0, op:1, fillcol:#334155, bgcol:#f1f5f9, margins:2",
            &sized.to_string_lossy(),
            &stamped.to_string_lossy(),
        ],
    )?;

    Ok(stamped)
}

/// Re-encode a vault image for the chosen preset.
fn recode(src: &Path, dst: &Path, long_edge: u32, quality: u8, grayscale: bool) -> Result<(), String> {
    let img = image::open(src).map_err(|e| format!("cannot read {src:?}: {e}"))?;

    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > long_edge {
        let scale = long_edge as f32 / w.max(h) as f32;
        img.resize(
            ((w as f32 * scale) as u32).max(1),
            ((h as f32 * scale) as u32).max(1),
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };

    // Grayscale roughly halves the bytes and costs nothing on a printed report.
    let img = if grayscale {
        image::DynamicImage::ImageLuma8(img.to_luma8())
    } else {
        image::DynamicImage::ImageRgb8(img.to_rgb8())
    };

    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode_image(&img)
        .map_err(|e| format!("cannot encode: {e}"))?;
    std::fs::write(dst, out).map_err(|e| format!("cannot write {dst:?}: {e}"))?;
    Ok(())
}

fn page_count(exe: &Path, pdf: &Path) -> u32 {
    run_pdfcpu(exe, &["info", "--json", &pdf.to_string_lossy()])
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["infos"][0]["pageCount"].as_u64())
        .unwrap_or(1) as u32
}

/// Build the export. Returns one part, or several when the size budget forces a
/// split — always at record boundaries, never mid-document.
pub fn build(
    conn: &Connection,
    user_id: &str,
    vault_root: &Path,
    work_root: &Path,
    req: &ExportRequest,
) -> Result<ExportResult, String> {
    let exe = pdfcpu_path()?;
    let docs = select_documents(conn, user_id, req)?;
    if docs.is_empty() {
        return Err("Nothing matches those filters.".into());
    }

    let work = work_root.join(format!("export-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&work).map_err(|e| format!("cannot create work dir: {e}"))?;

    let mut prepared: Vec<(PathBuf, u64)> = Vec::new();
    let mut missing = Vec::new();
    let mut undated = 0u32;

    for (i, doc) in docs.iter().enumerate() {
        if doc.doc_date.starts_with("0000") {
            undated += 1;
        }
        match prepare(&exe, vault_root, &work, doc, i, req.preset) {
            Ok(p) => {
                let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                prepared.push((p, bytes));
            }
            // One unreadable file must not cost the export. Report it instead.
            Err(e) => missing.push(format!("{} — {e}", doc.title)),
        }
    }

    if prepared.is_empty() {
        return Err(format!(
            "None of the selected documents could be read:\n{}",
            missing.join("\n")
        ));
    }

    // Pack greedily into parts, splitting only between documents.
    let budget = req.max_bytes.unwrap_or(u64::MAX);
    let mut groups: Vec<Vec<PathBuf>> = vec![Vec::new()];
    let mut group_bytes = 0u64;
    for (path, bytes) in prepared {
        let current = groups.last_mut().expect("at least one group");
        if !current.is_empty() && group_bytes + bytes > budget {
            groups.push(vec![path]);
            group_bytes = bytes;
        } else {
            current.push(path);
            group_bytes += bytes;
        }
    }

    std::fs::create_dir_all(&req.out_dir).map_err(|e| format!("cannot create output folder: {e}"))?;
    let multi = groups.len() > 1;
    let mut parts = Vec::new();

    for (i, group) in groups.iter().enumerate() {
        let name = if multi {
            format!("{} (part {} of {}).pdf", req.base_name, i + 1, groups.len())
        } else {
            format!("{}.pdf", req.base_name)
        };
        let merged = work.join(format!("merged-{i}.pdf"));
        let final_path = PathBuf::from(&req.out_dir).join(&name);

        let mut args: Vec<String> = vec![
            "merge".into(),
            "--bookmarks".into(),
            merged.to_string_lossy().to_string(),
        ];
        args.extend(group.iter().map(|p| p.to_string_lossy().to_string()));
        run_pdfcpu(&exe, &args.iter().map(String::as_str).collect::<Vec<_>>())?;

        run_pdfcpu(
            &exe,
            &["optimize", &merged.to_string_lossy(), &final_path.to_string_lossy()],
        )?;

        parts.push(ExportPart {
            bytes: std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0),
            pages: page_count(&exe, &final_path),
            documents: group.len() as u32,
            path: final_path.display().to_string(),
        });
    }

    let _ = std::fs::remove_dir_all(&work);

    Ok(ExportResult {
        total_documents: docs.len() as u32,
        undated,
        missing,
        parts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use rusqlite::params;

    /// In tests `current_exe` is the test harness under target/debug/deps, where
    /// the sidecar is not copied. Point at the checked-in binary instead.
    fn use_bundled_pdfcpu() {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join("pdfcpu-x86_64-pc-windows-msvc.exe");
        std::env::set_var("PDFCPU_PATH", p);
    }

    struct Fx {
        dir: PathBuf,
        conn: Connection,
        user: String,
        patients: Vec<(String, String)>,
    }

    impl Fx {
        fn new(name: &str) -> Self {
            use_bundled_pdfcpu();
            let dir = std::env::temp_dir().join(format!("mrt-exp-{name}-{}", ulid::Ulid::new()));
            std::fs::create_dir_all(dir.join("vault")).unwrap();
            std::fs::create_dir_all(dir.join("work")).unwrap();
            std::fs::create_dir_all(dir.join("out")).unwrap();
            let conn = crate::db::open(&dir.join("app.db")).unwrap();
            let user = crate::db::ensure_user(&conn).unwrap();
            Fx { dir, conn, user, patients: Vec::new() }
        }

        fn vault(&self) -> PathBuf { self.dir.join("vault") }

        fn patient(&mut self, name: &str) -> String {
            let id = ulid::Ulid::new().to_string();
            let slug = crate::naming::patient_slug(name);
            self.conn.execute(
                "INSERT INTO patients (id, owner_user_id, display_name, folder_slug, created_at, updated_at)
                 VALUES (?1,?2,?3,?4, datetime('now'), datetime('now'))",
                params![id, self.user, name, slug],
            ).unwrap();
            self.patients.push((id.clone(), slug));
            id
        }

        /// Write a real file into the vault and record its document row.
        fn doc(&self, patient_id: &str, date: &str, title: &str, kind: &str, px: u32) -> String {
            let (_, slug) = self.patients.iter().find(|(id, _)| id == patient_id).unwrap();
            let year = crate::naming::year_folder(date);
            let ext = if kind == "pdf" { "pdf" } else { "jpg" };
            let file = format!("{date}_{slug}_{}.{ext}", crate::naming::title_slug(title));
            let rel = format!("{slug}\\{year}\\{file}");
            let abs = self.vault().join(slug).join(&year).join(&file);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();

            if kind == "pdf" {
                // A genuine one-page PDF, produced by pdfcpu itself.
                let exe = pdfcpu_path().unwrap();
                let tmp_img = self.dir.join("tmp-src.jpg");
                write_jpeg(&tmp_img, px, px * 3 / 2);
                run_pdfcpu(&exe, &["import", "f:A4, pos:c",
                                   &abs.to_string_lossy(), &tmp_img.to_string_lossy()]).unwrap();
            } else {
                write_jpeg(&abs, px, px * 3 / 2);
            }

            let id = ulid::Ulid::new().to_string();
            self.conn.execute(
                "INSERT INTO documents (id, owner_user_id, patient_id, doc_date, date_source, title,
                                        doc_type, page_count, rel_path, sha256, byte_size, file_kind,
                                        created_at, updated_at)
                 VALUES (?1,?2,?3,?4,'manual',?5,'report',1,?6,'sha',1,?7, datetime('now'), datetime('now'))",
                params![id, self.user, patient_id, date, title, rel,
                        if kind == "pdf" { "pdf" } else { "jpeg" }],
            ).unwrap();
            id
        }

        fn request(&self, preset: Preset) -> ExportRequest {
            ExportRequest {
                patient_ids: vec![], years: vec![], category_ids: vec![], doc_types: vec![],
                preset, max_bytes: None,
                out_dir: self.dir.join("out").display().to_string(),
                base_name: "Medical History".into(),
            }
        }

        fn build(&self, req: &ExportRequest) -> Result<ExportResult, String> {
            build(&self.conn, &self.user, &self.vault(), &self.dir.join("work"), req)
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
    }

    fn write_jpeg(path: &Path, w: u32, h: u32) {
        let mut img = RgbImage::from_pixel(w, h, Rgb([250, 250, 250]));
        // Some structure so the encoder cannot collapse it to nothing.
        for y in 0..h {
            for x in 0..w {
                if (x / 8 + y / 8) % 2 == 0 {
                    img.put_pixel(x, y, Rgb([((x * 7) % 255) as u8, 40, 90]));
                }
            }
        }
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn page_sizes(pdf: &Path) -> Vec<(f64, f64)> {
        let exe = pdfcpu_path().unwrap();
        let out = run_pdfcpu(&exe, &["info", "--json", &pdf.to_string_lossy()]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        v["infos"][0]["pageSizes"].as_array().map(|a| {
            a.iter().map(|s| (s["width"].as_f64().unwrap_or(0.0), s["height"].as_f64().unwrap_or(0.0))).collect()
        }).unwrap_or_default()
    }

    #[test]
    fn mixed_images_and_pdfs_merge_into_one_uniform_a4_document() {
        let mut f = Fx::new("merge");
        let p = f.patient("Rahim Uddin");
        f.doc(&p, "2026-01-10", "CBC", "jpeg", 400);
        f.doc(&p, "2026-02-20", "Lipid Profile", "pdf", 400);
        f.doc(&p, "2026-03-14", "Thyroid Profile", "jpeg", 400);

        let res = f.build(&f.request(Preset::Standard)).unwrap();
        assert_eq!(res.parts.len(), 1);
        assert_eq!(res.total_documents, 3);
        assert!(res.missing.is_empty());

        let out = PathBuf::from(&res.parts[0].path);
        assert!(out.exists());
        assert_eq!(res.parts[0].pages, 3);

        // A single entry means every page is the same size — the property that
        // stops a viewer jumping and rezooming between records.
        let sizes = page_sizes(&out);
        assert_eq!(sizes.len(), 1, "pages must be uniform, got {sizes:?}");
        let (w, h) = sizes[0];
        assert!((w - 595.0).abs() < 2.0 && (h - 842.0).abs() < 2.0, "expected A4 portrait, got {w}x{h}");

        // The per-page header band is what actually delivers the scroll-through
        // experience — bookmarks are ignored by WhatsApp's preview and several
        // Android viewers, so its loss must fail a test rather than a doctor.
        let exe = pdfcpu_path().unwrap();
        let info = run_pdfcpu(&exe, &["info", "--json", &out.to_string_lossy()]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&info).unwrap();
        assert_eq!(v["infos"][0]["watermarked"].as_bool(), Some(true), "header band missing");
        assert_eq!(v["infos"][0]["bookmarks"].as_bool(), Some(true), "outline missing");
    }

    #[test]
    fn documents_are_ordered_by_date_with_undated_last() {
        let mut f = Fx::new("order");
        let p = f.patient("Rahim");
        f.doc(&p, "2026-03-14", "March", "jpeg", 200);
        f.doc(&p, "0000-00-00", "Unknown date", "jpeg", 200);
        f.doc(&p, "2024-01-05", "Older", "jpeg", 200);

        let docs = select_documents(&f.conn, &f.user, &f.request(Preset::Standard)).unwrap();
        assert_eq!(
            docs.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(),
            vec!["Older", "March", "Unknown date"],
            "undated must not sort to the front of a decade of history",
        );

        let res = f.build(&f.request(Preset::Standard)).unwrap();
        assert_eq!(res.undated, 1, "the user must be told some documents are undated");
    }

    #[test]
    fn the_email_safe_preset_is_substantially_smaller_than_original() {
        let mut f = Fx::new("preset");
        let p = f.patient("Rahim");
        for i in 1..=3 {
            f.doc(&p, &format!("2026-03-0{i}"), &format!("Report {i}"), "jpeg", 1600);
        }

        let big = f.build(&ExportRequest { base_name: "big".into(), ..f.request(Preset::Original) }).unwrap();
        let small = f.build(&ExportRequest { base_name: "small".into(), ..f.request(Preset::EmailSafe) }).unwrap();

        assert!(
            small.parts[0].bytes < big.parts[0].bytes,
            "email-safe ({}) must be smaller than original ({})",
            small.parts[0].bytes, big.parts[0].bytes,
        );
        assert_eq!(small.parts[0].pages, big.parts[0].pages);
    }

    #[test]
    fn an_oversized_export_splits_at_record_boundaries() {
        let mut f = Fx::new("split");
        let p = f.patient("Rahim");
        for i in 1..=4 {
            f.doc(&p, &format!("2026-03-0{i}"), &format!("Report {i}"), "jpeg", 900);
        }

        // A budget far below one document forces one document per part, proving the
        // split never lands inside a record.
        let req = ExportRequest { max_bytes: Some(1), ..f.request(Preset::Standard) };
        let res = f.build(&req).unwrap();

        assert_eq!(res.parts.len(), 4, "one document per part");
        assert!(res.parts.iter().all(|p| p.documents == 1));
        for (i, part) in res.parts.iter().enumerate() {
            assert!(
                part.path.contains(&format!("part {} of 4", i + 1)),
                "parts must be named so the doctor knows the order: {}",
                part.path,
            );
            assert!(PathBuf::from(&part.path).exists());
        }
    }

    #[test]
    fn a_file_missing_from_the_vault_is_reported_without_losing_the_export() {
        let mut f = Fx::new("missing");
        let p = f.patient("Rahim");
        f.doc(&p, "2026-01-10", "Present", "jpeg", 300);
        f.doc(&p, "2026-02-10", "Deleted In Explorer", "jpeg", 300);

        let rel: String = f.conn.query_row(
            "SELECT rel_path FROM documents WHERE title = 'Deleted In Explorer'", [], |r| r.get(0),
        ).unwrap();
        std::fs::remove_file(f.vault().join(rel.replace('\\', "/"))).unwrap();

        let res = f.build(&f.request(Preset::Standard)).unwrap();
        assert_eq!(res.parts.len(), 1);
        assert_eq!(res.parts[0].pages, 1, "the readable document still exports");
        assert_eq!(res.missing.len(), 1);
        assert!(res.missing[0].contains("Deleted In Explorer"));
    }

    #[test]
    fn filters_narrow_the_export_by_patient_and_year() {
        let mut f = Fx::new("filter");
        let rahim = f.patient("Rahim");
        let karim = f.patient("Karim");
        f.doc(&rahim, "2026-03-14", "Rahim 2026", "jpeg", 200);
        f.doc(&rahim, "2024-05-02", "Rahim 2024", "jpeg", 200);
        f.doc(&karim, "2026-06-01", "Karim 2026", "jpeg", 200);

        let by_patient = ExportRequest { patient_ids: vec![rahim.clone()], ..f.request(Preset::Standard) };
        assert_eq!(select_documents(&f.conn, &f.user, &by_patient).unwrap().len(), 2);

        let by_year = ExportRequest { years: vec!["2026".into()], ..f.request(Preset::Standard) };
        assert_eq!(select_documents(&f.conn, &f.user, &by_year).unwrap().len(), 2);

        let both = ExportRequest {
            patient_ids: vec![rahim], years: vec!["2026".into()], ..f.request(Preset::Standard)
        };
        let docs = select_documents(&f.conn, &f.user, &both).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title, "Rahim 2026");
    }

    /// Requirement 5: everything tagged "thyroid", across patients, ordered by date.
    #[test]
    fn exporting_one_category_crosses_patients_and_years() {
        let mut f = Fx::new("category");
        let rahim = f.patient("Rahim");
        let karim = f.patient("Karim");

        let thyroid = crate::categories::create(&f.conn, &f.user, "Thyroid", None).unwrap();
        let diabetes = crate::categories::create(&f.conn, &f.user, "Diabetes", None).unwrap();

        let a = f.doc(&rahim, "2024-02-01", "TSH Rahim", "jpeg", 200);
        let b = f.doc(&karim, "2026-05-09", "TSH Karim", "jpeg", 200);
        let c = f.doc(&rahim, "2025-06-06", "Fasting Glucose", "jpeg", 200);
        // Belongs to both — a folder tree could not express this.
        let d = f.doc(&rahim, "2025-01-01", "Combined Panel", "jpeg", 200);

        crate::categories::set_for_document(&mut f.conn, &f.user, &a, &[thyroid.id.clone()]).unwrap();
        crate::categories::set_for_document(&mut f.conn, &f.user, &b, &[thyroid.id.clone()]).unwrap();
        crate::categories::set_for_document(&mut f.conn, &f.user, &c, &[diabetes.id.clone()]).unwrap();
        crate::categories::set_for_document(&mut f.conn, &f.user, &d, &[thyroid.id.clone(), diabetes.id.clone()]).unwrap();

        let req = ExportRequest { category_ids: vec![thyroid.id.clone()], ..f.request(Preset::Standard) };
        let docs = select_documents(&f.conn, &f.user, &req).unwrap();

        assert_eq!(
            docs.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(),
            vec!["TSH Rahim", "Combined Panel", "TSH Karim"],
            "one category, both patients, date order",
        );

        let res = f.build(&req).unwrap();
        assert_eq!(res.parts[0].pages, 3);
        assert_eq!(res.total_documents, 3);
    }

    #[test]
    fn selecting_two_categories_returns_their_union_without_duplicates() {
        let mut f = Fx::new("cat-union");
        let p = f.patient("Rahim");
        let thyroid = crate::categories::create(&f.conn, &f.user, "Thyroid", None).unwrap();
        let diabetes = crate::categories::create(&f.conn, &f.user, "Diabetes", None).unwrap();

        let both = f.doc(&p, "2026-01-01", "Combined Panel", "jpeg", 200);
        let only_t = f.doc(&p, "2026-02-01", "TSH", "jpeg", 200);
        crate::categories::set_for_document(&mut f.conn, &f.user, &both, &[thyroid.id.clone(), diabetes.id.clone()]).unwrap();
        crate::categories::set_for_document(&mut f.conn, &f.user, &only_t, &[thyroid.id.clone()]).unwrap();

        let req = ExportRequest {
            category_ids: vec![thyroid.id, diabetes.id],
            ..f.request(Preset::Standard)
        };
        let docs = select_documents(&f.conn, &f.user, &req).unwrap();
        assert_eq!(docs.len(), 2, "a document in both categories must appear once, not twice");
    }

    #[test]
    fn an_empty_selection_fails_loudly_instead_of_writing_an_empty_pdf() {
        let mut f = Fx::new("empty");
        let p = f.patient("Rahim");
        f.doc(&p, "2026-03-14", "CBC", "jpeg", 200);

        let req = ExportRequest { years: vec!["1999".into()], ..f.request(Preset::Standard) };
        assert!(f.build(&req).unwrap_err().contains("Nothing matches"));
    }

    #[test]
    fn dates_are_displayed_day_first_even_though_storage_is_iso() {
        assert_eq!(display_date("2026-03-14"), "14/03/2026");
        assert_eq!(display_date("2026-03-00"), "03/2026");
        assert_eq!(display_date("0000-00-00"), "date unknown");
    }
}
