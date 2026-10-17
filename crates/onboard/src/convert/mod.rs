mod lighting;

pub use lighting::{MAX_EFFECTS, convert_layers};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub skipped: Vec<String>,
    pub approximated: Vec<String>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.skipped.is_empty() && self.approximated.is_empty()
    }

    pub fn lines(&self) -> Vec<String> {
        let skipped = self.skipped.iter().map(|line| format!("skipped: {line}"));
        let approximated = self.approximated.iter().map(|line| format!("approximated: {line}"));
        skipped.chain(approximated).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_list_skipped_before_approximated() {
        let report = Report { skipped: vec!["a".into()], approximated: vec!["b".into()] };
        assert_eq!(report.lines(), ["skipped: a", "approximated: b"]);
        assert!(!report.is_empty());
        assert!(Report::default().is_empty());
    }
}
