//! Windows only, and leaves a file in the Recycle Bin: run with `--ignored`.

#[cfg(windows)]
#[test]
#[ignore = "puts a file in the Recycle Bin"]
fn a_trashed_file_leaves_its_folder() {
    let path = std::env::temp_dir().join(format!("cb-trash-test-{}.png", std::process::id()));
    std::fs::write(&path, "pixels").unwrap();
    cb_platform::move_to_trash(&path).unwrap();
    assert!(!path.exists());
    assert!(cb_platform::move_to_trash(&path).is_err(), "a missing file is an error");
}
