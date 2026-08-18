//! 群文件的落盘与容量。

use crate::platforms::onebot::*;

#[test]
fn sanitizes_file_names() {
    assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
    assert_eq!(sanitize_file_name("C:\\evil\\x.exe"), "x.exe");
    assert_eq!(sanitize_file_name(".."), "file");
    assert_eq!(sanitize_file_name("  "), "file");
    assert_eq!(sanitize_file_name("报告 v2.pdf"), "报告 v2.pdf");
}

#[tokio::test]
async fn concurrent_inbound_files_with_the_same_name_do_not_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let first = save_platform_file(temp.path(), "report.txt", b"first");
    let second = save_platform_file(temp.path(), "report.txt", b"second");
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap();
    let second = second.unwrap();

    assert_ne!(first, second);
    let mut contents = vec![
        tokio::fs::read(first).await.unwrap(),
        tokio::fs::read(second).await.unwrap(),
    ];
    contents.sort();
    assert_eq!(contents, vec![b"first".to_vec(), b"second".to_vec()]);
}

#[tokio::test]
async fn inbound_file_store_enforces_a_total_capacity() {
    let temp = tempfile::tempdir().unwrap();
    save_platform_file(temp.path(), "existing.bin", b"12345678")
        .await
        .unwrap();

    assert!(
        ensure_platform_file_capacity(temp.path(), 2, 10, 10, Duration::from_secs(60),)
            .await
            .is_ok()
    );
    assert!(
        ensure_platform_file_capacity(temp.path(), 3, 10, 10, Duration::from_secs(60),)
            .await
            .is_err()
    );
}
