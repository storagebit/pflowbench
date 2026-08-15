// parse.rs -- comma-separated list parsing for the app's form fields.
//
// The temps and flows fields arrive from the UI as free text; these reject
// anything that is not a clean number rather than guessing.

/// Parse a comma-separated list of integers (e.g. a temps field from the UI).
pub fn parse_i64_list(s: &str) -> Result<Vec<i64>, String> {
    s.split(',')
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().parse::<i64>().map_err(|_| format!("not an integer: {}", t.trim())))
        .collect()
}

/// Parse a comma-separated list of floats (e.g. a flows field from the UI).
pub fn parse_f64_list(s: &str) -> Result<Vec<f64>, String> {
    s.split(',')
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().parse::<f64>().map_err(|_| format!("not a number: {}", t.trim())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lists() {
        assert_eq!(parse_i64_list("255, 265,275").unwrap(), vec![255, 265, 275]);
        assert_eq!(parse_f64_list("8,10, 12.5").unwrap(), vec![8.0, 10.0, 12.5]);
        assert!(parse_i64_list("255,not-a-number").is_err());
        assert!(parse_f64_list("").unwrap().is_empty());
    }
}
