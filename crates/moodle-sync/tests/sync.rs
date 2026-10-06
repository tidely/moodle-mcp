//! End-to-end sync against a minimal mock Moodle served over local HTTP.

use std::path::Path;

use moodle_api::Client;
use std::sync::atomic::Ordering;

use moodle_sync::{Course, FileState, Manifest, Options};

use moodle_mock::{TOKEN, mock_moodle};

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[tokio::test]
async fn syncs_course_and_downloads_skipped_files() {
    let mock = mock_moodle().await;
    let base = mock.base.clone();
    let client = Client::new(&base, TOKEN).unwrap();
    let root = std::env::temp_dir().join(format!("moodle-sync-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let opts = Options {
        max_file_size: Some(50),
        concurrency: 2,
    };

    let course = Course::fetch(&client, 42).await.unwrap();
    assert_eq!(course.dir_name(), "DB_2026 (42)");
    let dir = root.join(course.dir_name());
    let report = course.sync(&client, &dir, &opts).await.unwrap();

    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    // brief.pdf, small.pdf, the assignment's linked secret.pdf, three course page images.
    assert_eq!(report.downloaded.len(), 6, "{:?}", report.downloaded);
    assert_eq!(report.skipped.len(), 1);
    let assign = dir.join("Project 2 (1337)");
    let slides = dir.join("Slides (1400)");
    assert_eq!(read(&assign.join("brief.pdf")), "xxxxx");
    assert!(slides.join("small.pdf").is_file());
    assert!(!slides.join("big.mp4").exists());

    let course_index = read(&dir.join("index.md"));
    assert!(
        course_index.contains("Welcome to **DB**."),
        "{course_index}"
    );
    assert!(
        course_index.contains("Read the brief first."),
        "{course_index}"
    );
    assert!(
        course_index.contains("(_embedded/logo.png)"),
        "{course_index}"
    );
    // The section and the label both embed an `image.png`; both get disambiguated.
    assert_eq!(
        course_index.matches("(_embedded/image-").count(),
        2,
        "{course_index}"
    );
    assert_eq!(std::fs::read_dir(dir.join("_embedded")).unwrap().count(), 3);
    assert!(
        course_index.contains(
            "[Project 2](<Project 2 (1337)/index.md>) (assign) · Due: 2023-11-14 22:13 UTC"
        ),
        "{course_index}"
    );

    let assign_index = read(&assign.join("index.md"));
    assert!(
        assign_index.contains("[Brief](brief.pdf)"),
        "{assign_index}"
    );
    assert!(
        assign_index.contains("[other](_embedded/secret.pdf)"),
        "{assign_index}"
    );
    assert!(
        assign_index.contains("- [_embedded/secret.pdf](<_embedded/secret.pdf>) (5 B)"),
        "{assign_index}"
    );
    assert!(
        assign_index.contains("- Status: submitted"),
        "{assign_index}"
    );
    assert!(
        assign_index.contains("- [brief.pdf](<brief.pdf>)"),
        "{assign_index}"
    );

    let slides_index = read(&slides.join("index.md"));
    assert!(
        slides_index.contains("big.mp4 (100 B): not downloaded, over the size limit"),
        "{slides_index}"
    );

    for index in [&course_index, &assign_index, &slides_index] {
        assert!(!index.contains("pluginfile.php"), "{index}");
        assert!(!index.contains(TOKEN), "{index}");
    }

    // The manifest records what was written and what was skipped.
    let manifest = Manifest::load(&dir).expect("manifest");
    assert_eq!(manifest.course_id, 42);
    assert!(manifest.synced_at > 0);
    let state = |p: &str| manifest.files.iter().find(|f| f.path == p).map(|f| f.state);
    assert_eq!(state("Slides (1400)/big.mp4"), Some(FileState::Skipped));
    assert_eq!(state("Slides (1400)/small.pdf"), Some(FileState::Present));
    assert_eq!(state("_embedded/logo.png"), Some(FileState::Present));

    // Second sync keeps unchanged files.
    let report = course.sync(&client, &dir, &opts).await.unwrap();
    assert!(report.downloaded.is_empty(), "{:?}", report.downloaded);
    assert!(report.removed.is_empty(), "{:?}", report.removed);
    assert_eq!(report.unchanged, 6);

    // Explicit download ignores the size limit, accepts names or paths, and
    // updates the index and manifest.
    let names = ["/big.mp4".to_owned(), "nope.txt".to_owned()];
    let report = course
        .download(&client, &dir, 1400, Some(&names), &opts)
        .await
        .unwrap();
    assert_eq!(report.downloaded, [slides.join("big.mp4")]);
    assert_eq!(report.missing, ["nope.txt"]);
    assert!(read(&slides.join("index.md")).contains("- [big.mp4](<big.mp4>) (100 B)"));
    let manifest = Manifest::load(&dir).unwrap();
    let big = manifest
        .files
        .iter()
        .find(|f| f.path == "Slides (1400)/big.mp4");
    assert_eq!(big.map(|f| f.state), Some(FileState::Present));

    // When an activity disappears from Moodle, the next sync removes what it
    // wrote for it, but leaves the user's own files alone.
    std::fs::write(dir.join("notes.md"), "mine").unwrap();
    std::fs::write(slides.join("my-notes.txt"), "mine").unwrap();
    mock.slides_removed.store(true, Ordering::SeqCst);
    let course = Course::fetch(&client, 42).await.unwrap();
    let report = course.sync(&client, &dir, &opts).await.unwrap();
    let mut removed: Vec<_> = (report.removed.iter())
        .map(|p| p.strip_prefix(&dir).unwrap().to_owned())
        .collect();
    removed.sort();
    assert_eq!(
        removed,
        [
            Path::new("Slides (1400)/big.mp4"),
            Path::new("Slides (1400)/index.md"),
            Path::new("Slides (1400)/small.pdf"),
        ]
    );
    assert!(slides.join("my-notes.txt").is_file());
    assert!(dir.join("notes.md").is_file());
    assert!(!read(&dir.join("index.md")).contains("Slides"));

    // Once the user's file is gone too, the empty directory goes away.
    std::fs::remove_file(slides.join("my-notes.txt")).unwrap();
    mock.slides_removed.store(false, Ordering::SeqCst);
    let course = Course::fetch(&client, 42).await.unwrap();
    course.sync(&client, &dir, &opts).await.unwrap();
    mock.slides_removed.store(true, Ordering::SeqCst);
    let course = Course::fetch(&client, 42).await.unwrap();
    course.sync(&client, &dir, &opts).await.unwrap();
    assert!(!slides.exists());

    std::fs::remove_dir_all(&root).unwrap();
}
