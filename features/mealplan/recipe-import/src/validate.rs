//! Cooklang validation — the same parser configuration as
//! `cookbook::parse` (all extensions, bundled units), with the
//! parser's rendered report (codesnake line context) as the error.
//! Both synthesizers and the CLI gate on this.

/// Parse `source` as cooklang. `Err` carries the parser's rendered
/// diagnostics — exactly what gets fed back to the LLM on retry.
pub fn validate_cook(file_label: &str, source: &str) -> Result<(), String> {
    let parser =
        cooklang::CooklangParser::new(cooklang::Extensions::all(), cooklang::Converter::bundled());
    match parser.parse(source).into_result() {
        Ok(_) => Ok(()),
        Err(report) => {
            let mut buf = Vec::new();
            if report.write(file_label, source, false, &mut buf).is_err() {
                return Err(format!("{report}"));
            }
            Err(String::from_utf8_lossy(&buf).into_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_cooklang_passes() {
        let src = "---\ntitle: Toast\n---\n\nToast the @bread{2%slices} in a #toaster.\n";
        assert!(validate_cook("toast.cook", src).is_ok());
    }

    #[test]
    fn broken_cooklang_reports_with_context() {
        // Empty quantity before `%` → hard parse error (the parser is
        // otherwise remarkably lenient — unclosed braces just become
        // text).
        let src = "Add @flour{%} to the bowl\n\nand @sugar{1%cup}.\n";
        let report = validate_cook("broken.cook", src).unwrap_err();
        assert!(!report.is_empty());
    }
}
