use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use strata_dicom::series::SeriesManifest;

/// Row shape for `GET /api/series`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesSummary {
    pub series_uid: String,
    pub study_uid: String,
    pub patient_id: String,
    pub modality: String,
    pub rows: u16,
    pub cols: u16,
    pub series_description: Option<String>,
    pub study_description: Option<String>,
    pub slice_count: u32,
    pub is_volume: bool,
    pub hu_calibrated: bool,
    pub uniform_spacing: bool,
    pub spacing_mm: Option<f64>,
}

/// Row shape for `GET /api/series/:uid`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesDetail {
    pub series_uid: String,
    pub study_uid: String,
    pub patient_id: String,
    pub modality: String,
    pub rows: u16,
    pub cols: u16,
    pub series_description: Option<String>,
    pub study_description: Option<String>,
    pub slice_count: u32,
    pub is_volume: bool,
    pub hu_calibrated: bool,
    pub uniform_spacing: bool,
    pub spacing_mm: Option<f64>,
    pub pixel_spacing: Option<[f64; 2]>,
    pub slice_thickness: Option<f64>,
    pub warnings: Vec<String>,
    pub depths: Vec<f64>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS series (
    series_uid        TEXT PRIMARY KEY,
    study_uid         TEXT NOT NULL,
    patient_id        TEXT NOT NULL,
    modality          TEXT NOT NULL,
    rows              INTEGER NOT NULL,
    cols              INTEGER NOT NULL,
    series_description TEXT,
    study_description  TEXT,
    uniform_spacing   INTEGER NOT NULL,
    spacing_mm        REAL,
    hu_calibrated     INTEGER NOT NULL,
    is_volume         INTEGER NOT NULL,
    warnings_json      TEXT NOT NULL,
    pixel_spacing_row REAL,
    pixel_spacing_col REAL,
    slice_thickness   REAL
);

CREATE TABLE IF NOT EXISTS slices (
    series_uid TEXT NOT NULL,
    ordinal    INTEGER NOT NULL,
    path       TEXT NOT NULL,
    depth      REAL NOT NULL,
    PRIMARY KEY (series_uid, ordinal)
);
";

pub struct Index {
    conn: Connection,
}

/// `CREATE TABLE IF NOT EXISTS` doesn't add columns to a table that already
/// exists, so a `series` table left over from before `series_description`/
/// `study_description` existed would otherwise make every insert fail with
/// an opaque "no such column" at runtime. Detect that case up front and
/// drop the affected tables so `SCHEMA` recreates them cleanly — safe here
/// specifically because `main.rs` fully re-scans and re-inserts every
/// series on every startup, so this index is a rebuildable cache, not a
/// system of record.
fn migrate_if_needed(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(series)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let is_stale = !columns.is_empty() && !columns.iter().any(|c| c == "series_description");
    if is_stale {
        conn.execute_batch("DROP TABLE IF EXISTS series; DROP TABLE IF EXISTS slices;")?;
    }
    Ok(())
}

impl Index {
    pub fn open_in_memory() -> Result<Index> {
        let conn = Connection::open_in_memory()?;
        migrate_if_needed(&conn)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Index { conn })
    }

    pub fn open(path: &Path) -> Result<Index> {
        let conn = Connection::open(path)?;
        migrate_if_needed(&conn)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Index { conn })
    }

    /// Idempotent: replaces any existing series with the same `series_uid`
    /// rather than duplicating it. `slices` are persisted with an `ordinal`
    /// equal to their index in `manifest.slices`, which `SeriesManifest`
    /// already sorted by depth — that sort is the single source of truth for
    /// slice order and is never redone here or by any reader of this table.
    pub fn insert_series(&self, m: &SeriesManifest) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        tx.execute("DELETE FROM slices WHERE series_uid = ?1", [&m.series_uid])?;
        tx.execute(
            "INSERT OR REPLACE INTO series (
                series_uid, study_uid, patient_id, modality, rows, cols,
                series_description, study_description,
                uniform_spacing, spacing_mm, hu_calibrated, is_volume, warnings_json,
                pixel_spacing_row, pixel_spacing_col, slice_thickness
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            rusqlite::params![
                m.series_uid,
                m.study_uid,
                m.patient_id,
                m.modality,
                m.rows,
                m.cols,
                m.series_description,
                m.study_description,
                m.uniform_spacing,
                m.spacing_mm,
                m.hu_calibrated,
                m.is_volume,
                serde_json::to_string(&m.warnings)?,
                m.slices[0].pixel_spacing.map(|(r, _)| r),
                m.slices[0].pixel_spacing.map(|(_, c)| c),
                m.slices[0].slice_thickness,
            ],
        )?;

        let mut stmt = tx.prepare(
            "INSERT INTO slices (series_uid, ordinal, path, depth) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (ordinal, slice) in m.slices.iter().enumerate() {
            stmt.execute(rusqlite::params![
                m.series_uid,
                ordinal as u32,
                slice.path.to_string_lossy(),
                slice.depth,
            ])?;
        }
        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    pub fn list_series(&self) -> Result<Vec<SeriesSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.series_uid, s.study_uid, s.patient_id, s.modality, s.rows, s.cols,
                    s.series_description, s.study_description,
                    s.is_volume, s.hu_calibrated, s.uniform_spacing, s.spacing_mm,
                    (SELECT COUNT(*) FROM slices sl WHERE sl.series_uid = s.series_uid) AS slice_count
             FROM series s
             ORDER BY s.series_uid",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SeriesSummary {
                series_uid: row.get(0)?,
                study_uid: row.get(1)?,
                patient_id: row.get(2)?,
                modality: row.get(3)?,
                rows: row.get(4)?,
                cols: row.get(5)?,
                series_description: row.get(6)?,
                study_description: row.get(7)?,
                is_volume: row.get(8)?,
                hu_calibrated: row.get(9)?,
                uniform_spacing: row.get(10)?,
                spacing_mm: row.get(11)?,
                slice_count: row.get(12)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_series(&self, uid: &str) -> Result<Option<SeriesDetail>> {
        let row = self
            .conn
            .query_row(
                "SELECT s.series_uid, s.study_uid, s.patient_id, s.modality, s.rows, s.cols,
                        s.series_description, s.study_description,
                        s.is_volume, s.hu_calibrated, s.uniform_spacing, s.spacing_mm,
                        s.pixel_spacing_row, s.pixel_spacing_col, s.slice_thickness, s.warnings_json,
                        (SELECT COUNT(*) FROM slices sl WHERE sl.series_uid = s.series_uid) AS slice_count
                 FROM series s WHERE s.series_uid = ?1",
                [uid],
                |row| {
                    let pixel_spacing_row: Option<f64> = row.get(12)?;
                    let pixel_spacing_col: Option<f64> = row.get(13)?;
                    let warnings_json: String = row.get(15)?;
                    Ok((
                        SeriesDetail {
                            series_uid: row.get(0)?,
                            study_uid: row.get(1)?,
                            patient_id: row.get(2)?,
                            modality: row.get(3)?,
                            rows: row.get(4)?,
                            cols: row.get(5)?,
                            series_description: row.get(6)?,
                            study_description: row.get(7)?,
                            is_volume: row.get(8)?,
                            hu_calibrated: row.get(9)?,
                            uniform_spacing: row.get(10)?,
                            spacing_mm: row.get(11)?,
                            pixel_spacing: pixel_spacing_row
                                .zip(pixel_spacing_col)
                                .map(|(r, c)| [r, c]),
                            slice_thickness: row.get(14)?,
                            warnings: Vec::new(), // filled below
                            depths: Vec::new(),   // filled below
                            slice_count: row.get(16)?,
                        },
                        warnings_json,
                    ))
                },
            )
            .map(Some)
            .or_else(|e| {
                if e == rusqlite::Error::QueryReturnedNoRows {
                    Ok(None)
                } else {
                    Err(e)
                }
            })?;

        let Some((mut detail, warnings_json)) = row else {
            return Ok(None);
        };
        detail.warnings = serde_json::from_str(&warnings_json)?;

        let mut stmt = self
            .conn
            .prepare("SELECT depth FROM slices WHERE series_uid = ?1 ORDER BY ordinal")?;
        let depths = stmt
            .query_map([uid], |row| row.get::<_, f64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        detail.depths = depths;

        Ok(Some(detail))
    }

    /// All slice paths for a series, in ordinal (geometric) order, as one
    /// query. The volume pipeline needs every path up front to fan decoding
    /// out across threads; doing that via `slice_path` in a loop would be
    /// one locked SQLite round trip per slice instead of one for the whole
    /// series.
    pub fn slice_paths(&self, uid: &str) -> Result<Vec<PathBuf>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM slices WHERE series_uid = ?1 ORDER BY ordinal")?;
        let rows = stmt.query_map([uid], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    /// Looks up the on-disk path for one slice by its persisted geometric
    /// ordinal. Task 9's pixel-data endpoint uses this to fetch a file
    /// without re-scanning or re-deriving order.
    pub fn slice_path(&self, uid: &str, ordinal: u32) -> Result<Option<PathBuf>> {
        let path: Option<String> = self
            .conn
            .query_row(
                "SELECT path FROM slices WHERE series_uid = ?1 AND ordinal = ?2",
                rusqlite::params![uid, ordinal],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|e| {
                if e == rusqlite::Error::QueryReturnedNoRows {
                    Ok(None)
                } else {
                    Err(e)
                }
            })?;
        Ok(path.map(PathBuf::from))
    }

    pub fn series_count(&self) -> Result<u32> {
        let count: u32 = self
            .conn
            .query_row("SELECT COUNT(*) FROM series", [], |row| row.get(0))?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use strata_dicom::meta::SliceMeta;

    /// Builds a manifest by hand rather than importing strata-dicom's test
    /// fixture builder, which lives under that crate's own tests/ and isn't
    /// importable from here.
    fn make_slice(ordinal: i32, depth: f64, hu_calibrated: bool) -> SliceMeta {
        SliceMeta {
            path: PathBuf::from(format!("/data/slice-{ordinal}.dcm")),
            patient_id: "PAT1".to_string(),
            study_uid: "STUDY1".to_string(),
            series_uid: "SERIES1".to_string(),
            sop_uid: format!("SOP{ordinal}"),
            modality: "CT".to_string(),
            rows: 512,
            cols: 512,
            position: [0.0, 0.0, depth],
            orientation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            rescale: if hu_calibrated {
                Some((1.0, -1024.0))
            } else {
                None
            },
            pixel_spacing: Some((0.7, 0.7)),
            slice_thickness: Some(5.0),
            depth,
            series_description: None,
            study_description: None,
        }
    }

    /// `SeriesManifest::from_slices` is `pub(crate)` to strata-dicom and not
    /// callable from here, so this builds the manifest directly (every field
    /// is public) with slices already in the depth order that
    /// `from_slices` would have produced — that sort itself is strata-dicom's
    /// responsibility and is proven by its own tests, not re-tested here.
    fn make_manifest(hu_calibrated: bool) -> SeriesManifest {
        let slices = vec![
            make_slice(0, -344.0, hu_calibrated),
            make_slice(1, -339.0, hu_calibrated),
            make_slice(2, -334.0, hu_calibrated),
        ];
        SeriesManifest {
            series_uid: "SERIES1".to_string(),
            study_uid: "STUDY1".to_string(),
            patient_id: "PAT1".to_string(),
            modality: "CT".to_string(),
            rows: 512,
            cols: 512,
            series_description: None,
            study_description: None,
            uniform_spacing: true,
            spacing_mm: Some(5.0),
            hu_calibrated,
            is_volume: true,
            warnings: Vec::new(),
            slices,
        }
    }

    #[test]
    fn round_trips_a_manifest() {
        let idx = Index::open_in_memory().unwrap();
        let manifest = make_manifest(true);
        idx.insert_series(&manifest).unwrap();

        let detail = idx.get_series("SERIES1").unwrap().unwrap();
        assert_eq!(detail.slice_count, 3);
        assert_eq!(detail.depths, vec![-344.0, -339.0, -334.0]);
        assert_eq!(detail.series_uid, "SERIES1");
        assert_eq!(detail.study_uid, "STUDY1");
        assert_eq!(detail.patient_id, "PAT1");
        assert!(detail.hu_calibrated);
    }

    #[test]
    fn reindexing_replaces_rather_than_duplicating() {
        let idx = Index::open_in_memory().unwrap();
        let manifest = make_manifest(true);
        idx.insert_series(&manifest).unwrap();
        idx.insert_series(&manifest).unwrap();

        let list = idx.list_series().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].slice_count, 3);
    }

    #[test]
    fn slice_path_resolves_by_ordinal() {
        let idx = Index::open_in_memory().unwrap();
        let manifest = make_manifest(true);
        idx.insert_series(&manifest).unwrap();

        let path0 = idx.slice_path("SERIES1", 0).unwrap().unwrap();
        assert_eq!(path0, PathBuf::from("/data/slice-0.dcm"));

        let missing = idx.slice_path("SERIES1", 99).unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn slice_paths_returns_all_in_ordinal_order() {
        let idx = Index::open_in_memory().unwrap();
        let manifest = make_manifest(true);
        idx.insert_series(&manifest).unwrap();

        let paths = idx.slice_paths("SERIES1").unwrap();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/data/slice-0.dcm"),
                PathBuf::from("/data/slice-1.dcm"),
                PathBuf::from("/data/slice-2.dcm"),
            ]
        );
    }

    #[test]
    fn unknown_series_returns_none_not_error() {
        let idx = Index::open_in_memory().unwrap();
        assert!(idx.get_series("nope").unwrap().is_none());
    }

    #[test]
    fn hu_calibrated_false_is_preserved() {
        let idx = Index::open_in_memory().unwrap();
        let manifest = make_manifest(false);
        idx.insert_series(&manifest).unwrap();

        let detail = idx.get_series("SERIES1").unwrap().unwrap();
        assert!(!detail.hu_calibrated);

        let list = idx.list_series().unwrap();
        assert!(!list[0].hu_calibrated);
    }
}
