use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DicomError {
    #[error("ImageOrientationPatient has {0} values, expected 6")]
    BadOrientation(usize),

    #[error("row and column direction cosines are parallel, no defined slice normal")]
    DegenerateOrientation,

    #[allow(dead_code)]
    #[error("missing required tag {tag} in {file}", file = .file.display())]
    MissingTag { tag: String, file: PathBuf },

    #[allow(dead_code)]
    #[error("unsupported transfer syntax {uid} in {file}", file = .file.display())]
    UnsupportedTransferSyntax { uid: String, file: PathBuf },
}

pub type Result<T> = std::result::Result<T, DicomError>;
