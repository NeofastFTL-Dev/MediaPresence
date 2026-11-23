//! Pairing QR code: the module matrix as plain booleans, so the UI can draw it and the self-test
//! can verify it without any image crate.

use qrcode::types::Color;
use qrcode::QrCode;

/// Returns (width, modules row-major, true = dark) or an error message.
pub fn matrix(text: &str) -> Result<(usize, Vec<bool>), String> {
    let code = QrCode::new(text.as_bytes()).map_err(|e| format!("QR maken mislukt: {e}"))?;
    let width = code.width();
    let modules = code.to_colors().into_iter().map(|c| c == Color::Dark).collect();
    Ok((width, modules))
}
