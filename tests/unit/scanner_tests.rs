use super::{discover_files, manifest_path_for};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[test]
fn reuses_manifest_file_path_next_to_session_cache() {
    let manifest_path = manifest_path_for(Path::new("/tmp/session-cache-v1.bin"));
    assert_eq!(
        manifest_path,
        PathBuf::from("/tmp/session-cache-v1-manifest.bin")
    );
}

#[test]
fn discovers_files_and_populates_manifest() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let session_root = temp_dir.path().join("sessions");
    let day_dir = session_root.join("2026/03/06");
    fs::create_dir_all(&day_dir).expect("mkdirs");
    fs::write(day_dir.join("rollout-1.jsonl"), "{}\n").expect("write file");
    let cache_path = temp_dir.path().join("session-cache.bin");

    let files = discover_files(&session_root, &cache_path, None, None, true).expect("discover");

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].relative_path, "2026/03/06/rollout-1.jsonl");
    assert!(manifest_path_for(&cache_path).exists());
}

#[test]
fn refreshes_manifest_file_metadata_for_past_directories() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let session_root = temp_dir.path().join("sessions");
    let day_dir = session_root.join("2000/01/01");
    fs::create_dir_all(&day_dir).expect("mkdirs");
    let file_path = day_dir.join("rollout-1.jsonl");
    fs::write(&file_path, "{}\n").expect("write file");
    let cache_path = temp_dir.path().join("session-cache.bin");

    let first = discover_files(&session_root, &cache_path, None, None, true).expect("first");
    thread::sleep(Duration::from_millis(10));
    fs::write(&file_path, "{\"a\":1}\n{\"b\":2}\n").expect("rewrite file");

    let second = discover_files(&session_root, &cache_path, None, None, true).expect("second");

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert!(second[0].file_size > first[0].file_size);
}
