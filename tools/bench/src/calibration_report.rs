//! Portable validation of calibration-only benchmark report JSON.

/// Return true only when a calibration-only report passed its floor and
/// contains no scored pair samples. S3-32a serializes its status as
/// `timing_status`, while older profiles use `status`.
pub fn calibration_only_report_succeeded(value: &serde_json::Value) -> bool {
    let sequences = value.get("sequences").and_then(serde_json::Value::as_array);
    let calibration_passed = value
        .get("calibration_floor_passed")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        || sequences.is_some_and(|items| {
            !items.is_empty()
                && items.iter().all(|item| {
                    item.get("calibration_floor_passed")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                })
        });
    let s3_32a_report = value.get("experiment").and_then(serde_json::Value::as_str)
        == Some("s3-32a-aliased-add-rax-rax-baseline-vs-candidate");
    let s3_32a_cases = value.get("cases").and_then(serde_json::Value::as_array);
    let no_s3_32a_samples = s3_32a_report
        && value
            .get("compile_pairs")
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty)
        && s3_32a_cases.is_some_and(|cases| {
            !cases.is_empty()
                && cases.iter().all(|case| {
                    case.get("native_pairs")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(Vec::is_empty)
                })
        });
    let no_samples = no_s3_32a_samples
        || value
            .get("sample_pairs")
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty)
        || sequences.is_some_and(|items| {
            !items.is_empty()
                && items.iter().all(|item| {
                    item.get("sample_pairs")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(Vec::is_empty)
                })
        });
    let status = value
        .get("status")
        .or_else(|| value.get("timing_status"))
        .and_then(serde_json::Value::as_str);
    status == Some("calibration_only_unscored") && calibration_passed && no_samples
}

#[cfg(test)]
mod tests {
    use super::calibration_only_report_succeeded;

    #[test]
    fn s3_32a_calibration_only_json_shape_is_portable_and_requires_no_samples() {
        let report = serde_json::json!({
            "timing_status": "calibration_only_unscored",
            "experiment": "s3-32a-aliased-add-rax-rax-baseline-vs-candidate",
            "calibration_floor_passed": true,
            "compile_pairs": [],
            "cases": [{"native_pairs": []}, {"native_pairs": []}]
        });
        assert!(report.get("status").is_none());
        assert!(calibration_only_report_succeeded(&report));

        let mut compile_sample = report.clone();
        compile_sample["compile_pairs"] = serde_json::json!([{"pair_index": 0}]);
        assert!(!calibration_only_report_succeeded(&compile_sample));

        let mut native_sample = report.clone();
        native_sample["cases"][0]["native_pairs"] = serde_json::json!([{"pair_index": 0}]);
        assert!(!calibration_only_report_succeeded(&native_sample));

        let mut failed_floor = report;
        failed_floor["calibration_floor_passed"] = serde_json::json!(false);
        failed_floor["timing_status"] = serde_json::json!("calibration_only_failed_unscored");
        assert!(!calibration_only_report_succeeded(&failed_floor));
    }
}
