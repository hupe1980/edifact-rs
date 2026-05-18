use edifact_rs::{
    ProfileRulePack, ValidationContext, ValidationIssue, ValidationSeverity, from_reader_iter,
};

fn main() -> Result<(), edifact_rs::EdifactError> {
    let input = b"\
UNH+1+ORDERS:D:11A:UN'\
BGM+220+PO-4711+9'\
NAD+BY+4000001000002::9'\
UNT+4+1'\
UNH+2+ORDERS:D:11A:UN'\
BGM+220+PO-4712+9'\
NAD+BY+4000001000002::9'\
UNT+4+2'";

    let pack = ProfileRulePack::builder("ORDERS-PROGRESSIVE")
        .for_message_type("ORDERS")
        .with_rule_fn(|segments| {
            let has_bgm = segments.iter().any(|segment| segment.tag == "BGM");
            (!has_bgm).then(|| {
                ValidationIssue::new(ValidationSeverity::Error, "BGM is required per streamed message window")
                    .with_rule_id("ORDERS-PROGRESSIVE-BGM")
                    .with_segment("BGM")
            })
        })
        .with_rule_fn(|segments| {
            let has_buyer = segments
                .iter()
                .filter(|segment| segment.tag == "NAD")
                .any(|segment| segment.element_str(0) == Some("BY"));
            (!has_buyer).then(|| {
                ValidationIssue::new(
                    ValidationSeverity::Warning,
                    "buyer NAD+BY is recommended for progressive ORDERS validation",
                )
                .with_rule_id("ORDERS-PROGRESSIVE-BUYER")
                .with_segment("NAD")
                .with_element_index(0)
            })
        });

    let context = ValidationContext::builder()
        .with_message_type("ORDERS")
        .with_profile_pack(pack)
        .build();

    let mut current_window = Vec::new();
    let mut in_message = false;
    let mut validated_windows = 0usize;

    for owned in from_reader_iter(std::io::Cursor::new(input.to_vec())) {
        let segment = owned?;

        if segment.tag == "UNH" {
            if in_message {
                return Err(edifact_rs::EdifactError::ValidationFailed {
                    error_count: 1,
                    first_message:
                        "UNH seen while already inside a message window (missing UNT before next UNH)"
                            .to_owned(),
                });
            }
            current_window.clear();
            in_message = true;
        }

        if in_message {
            current_window.push(segment);
        }

        let end_of_window = current_window
            .last()
            .is_some_and(|segment| segment.tag == "UNT");
        if end_of_window {
            {
                let borrowed: Vec<_> = current_window
                    .iter()
                    .map(|segment| segment.as_borrowed())
                    .collect();
                let report = context.validate_lenient(&borrowed);
                println!(
                    "window {}: {} issue(s)",
                    validated_windows + 1,
                    report.total_issues()
                );
                if report.has_errors() {
                    println!("{}", report.render_deterministic());
                }
            }

            validated_windows += 1;
            current_window.clear();
            in_message = false;
        }
    }

    if in_message {
        return Err(edifact_rs::EdifactError::ValidationFailed {
            error_count: 1,
            first_message: "unterminated streamed message window: missing UNT".to_owned(),
        });
    }

    println!("validated_windows={validated_windows}");
    Ok(())
}
