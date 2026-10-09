use crate::{CaptureBackend, CaptureError, CaptureKind, CaptureSourceSpec, DisplayInfo, Result};

/// Synthetic source: a moving GStreamer test pattern. Never presented to
/// users as a real display; it exists for tests and CI.
#[derive(Debug, Clone)]
pub struct TestPatternCapture {
    pub width: u32,
    pub height: u32,
}

impl Default for TestPatternCapture {
    fn default() -> Self {
        TestPatternCapture { width: 1280, height: 720 }
    }
}

impl CaptureBackend for TestPatternCapture {
    fn kind(&self) -> CaptureKind {
        CaptureKind::TestPattern
    }

    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(vec![DisplayInfo {
            id: "test-pattern".into(),
            name: "Test pattern (synthetic)".into(),
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
            primary: false,
            backend: CaptureKind::TestPattern,
        }])
    }

    fn open(&self, display_id: &str) -> Result<CaptureSourceSpec> {
        if display_id != "test-pattern" {
            return Err(CaptureError::UnknownDisplay(display_id.into()));
        }
        Ok(CaptureSourceSpec::TestPattern { width: self.width, height: self.height, pattern: "ball".into() })
    }
}
