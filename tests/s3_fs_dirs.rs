use std::path::PathBuf;
use std::sync::Arc;

use s3::{Auth, BlockingClient, Credentials};
use s3_fs::S3FileSystem;
use testcontainers::ContainerAsync;
use testcontainers_modules::{minio::MinIO, testcontainers::runners::AsyncRunner};
use virtual_fs::{FileSystem, FileType, Metadata};

async fn minio_s3_client(container: &ContainerAsync<MinIO>) -> BlockingClient {
    let port = container.get_host_port_ipv4(9000).await.unwrap();
    let endpoint = format!("http://127.0.0.1:{port}");

    let credentials = Credentials::new("minioadmin", "minioadmin").unwrap();
    BlockingClient::builder(&endpoint)
        .unwrap()
        .region("us-east-1")
        .auth(Auth::Static(credentials))
        .build()
        .unwrap()
}

#[tokio::test]
async fn test_create_dir() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "fs-test";

    let fs = S3FileSystem::init(bucket_name.to_owned(), client);
    fs.create_dir(&PathBuf::from("/test")).unwrap();
    fs.create_dir(&PathBuf::from("/test")).unwrap_err();
}

#[tokio::test]
async fn test_create_dir_nested() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "fs-test";

    let fs = S3FileSystem::init(bucket_name.to_owned(), client);
    fs.create_dir(&PathBuf::from("/test")).unwrap();
    fs.create_dir(&PathBuf::from("/test/it")).unwrap();
    fs.create_dir(&PathBuf::from("/test/it")).unwrap_err();
}

#[tokio::test]
async fn test_remove_dir() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "fs-test";

    let fs = S3FileSystem::init(bucket_name.to_owned(), client);
    fs.create_dir(&PathBuf::from("/test")).unwrap();
    fs.remove_dir(&PathBuf::from("/test")).unwrap();
    fs.create_dir(&PathBuf::from("/test")).unwrap();
}

#[tokio::test]
async fn test_remove_dir_nested() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "fs-test";

    let fs = S3FileSystem::init(bucket_name.to_owned(), client);
    fs.create_dir(&PathBuf::from("/test")).unwrap();
    fs.create_dir(&PathBuf::from("/test/it")).unwrap();
    fs.remove_dir(&PathBuf::from("/test/it")).unwrap();
    fs.create_dir(&PathBuf::from("/test/it")).unwrap();
}

#[tokio::test]
async fn test_remove_dir_non_empty() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "fs-test";

    let fs = S3FileSystem::init(bucket_name.to_owned(), client);
    fs.create_dir(&PathBuf::from("/test")).unwrap();
    fs.create_dir(&PathBuf::from("/test/it")).unwrap();
    fs.remove_dir(&PathBuf::from("/test")).unwrap_err();
}

#[tokio::test]
async fn test_dir_metadata() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "fs-test";

    let fs = S3FileSystem::init(bucket_name.to_owned(), client);
    fs.create_dir(&PathBuf::from("/test")).unwrap();
    let metadata = fs.metadata(&PathBuf::from("/test")).unwrap();
    let expected = Metadata {
        len: 0,
        ft: FileType::new_dir(),
        accessed: 0,
        created: metadata.created,
        modified: 0,
    };
    assert_eq!(metadata, expected);
}

#[tokio::test]
async fn test_dir_metadata_nested() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "fs-test";

    let fs = S3FileSystem::init(bucket_name.to_owned(), client);
    fs.create_dir(&PathBuf::from("/test")).unwrap();
    fs.create_dir(&PathBuf::from("/test/it")).unwrap();
    let metadata = fs.metadata(&PathBuf::from("/test/it")).unwrap();
    let expected = Metadata {
        len: 0,
        ft: FileType::new_dir(),
        accessed: 0,
        created: metadata.created,
        modified: 0,
    };
    assert_eq!(metadata, expected);
}

/// Many threads creating distinct children in the *same* parent directory race
/// on the parent object's CAS update. With the retry loop in `update_dir`, every
/// create must eventually succeed and no update may be lost.
#[tokio::test]
async fn test_concurrent_create_dir_same_parent() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = Arc::new(S3FileSystem::init("fs-cas".to_owned(), client));
    fs.create_dir(&PathBuf::from("/d")).unwrap();

    const N: usize = 12;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let fs = Arc::clone(&fs);
            std::thread::spawn(move || {
                fs.create_dir(&PathBuf::from(format!("/d/child{i}"))).unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    // All N children survived the concurrent CAS updates (none lost).
    let count = fs.read_dir(&PathBuf::from("/d")).unwrap().count();
    assert_eq!(count, N);
}

/// Regression test for the check-empty-then-unlink race in `remove_dir`.
///
/// One thread churns `/d` (remove then recreate) while another keeps creating
/// `/d/sub`. The invariant: whenever a `create_dir("/d/sub")` reports success,
/// `/d/sub` must actually exist (the worker can then read and remove it). With
/// the old single-CAS removal, the churner could unlink and delete `/d` right
/// after the worker's insert committed, losing it — the worker's asserts would
/// then fail.
#[tokio::test]
async fn test_remove_dir_vs_concurrent_create() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = Arc::new(S3FileSystem::init("fs-race".to_owned(), client));
    fs.create_dir(&PathBuf::from("/d")).unwrap();

    const K: usize = 25;

    let churner = {
        let fs = Arc::clone(&fs);
        std::thread::spawn(move || {
            for _ in 0..K {
                let _ = fs.remove_dir(&PathBuf::from("/d"));
                let _ = fs.create_dir(&PathBuf::from("/d"));
            }
        })
    };

    let worker = {
        let fs = Arc::clone(&fs);
        std::thread::spawn(move || {
            for _ in 0..K {
                if fs.create_dir(&PathBuf::from("/d/sub")).is_ok() {
                    // The insert committed, so `/d/sub` must be live: the
                    // churner cannot remove the now-non-empty `/d` underneath us.
                    fs.metadata(&PathBuf::from("/d/sub")).unwrap();
                    fs.remove_dir(&PathBuf::from("/d/sub")).unwrap();
                }
            }
        })
    };

    churner.join().unwrap();
    worker.join().unwrap();
}

/// Same-directory rename moves the entry, preserving its identity.
#[tokio::test]
async fn test_rename_same_dir() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-rename".to_owned(), client);
    fs.create_dir(&PathBuf::from("/a")).unwrap();
    fs.create_dir(&PathBuf::from("/a/b")).unwrap();
    let before = fs.metadata(&PathBuf::from("/a/b")).unwrap();

    fs.rename(&PathBuf::from("/a/b"), &PathBuf::from("/a/c"))
        .await
        .unwrap();

    // Old name is gone, new name is there with the same (preserved) metadata.
    fs.metadata(&PathBuf::from("/a/b")).unwrap_err();
    let after = fs.metadata(&PathBuf::from("/a/c")).unwrap();
    assert!(after.is_dir());
    assert_eq!(after.created, before.created);

    // The renamed directory is usable under its new path.
    fs.create_dir(&PathBuf::from("/a/c/inner")).unwrap();
}

/// Renaming a missing entry fails.
#[tokio::test]
async fn test_rename_missing_source() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-rename".to_owned(), client);
    fs.create_dir(&PathBuf::from("/a")).unwrap();

    let err = fs
        .rename(&PathBuf::from("/a/nope"), &PathBuf::from("/a/x"))
        .await
        .unwrap_err();
    assert_eq!(err, virtual_fs::FsError::EntryNotFound);
}

/// Renaming onto an existing name is rejected (no overwrite yet).
#[tokio::test]
async fn test_rename_dest_exists() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-rename".to_owned(), client);
    fs.create_dir(&PathBuf::from("/a")).unwrap();
    fs.create_dir(&PathBuf::from("/a/b")).unwrap();
    fs.create_dir(&PathBuf::from("/a/c")).unwrap();

    let err = fs
        .rename(&PathBuf::from("/a/b"), &PathBuf::from("/a/c"))
        .await
        .unwrap_err();
    assert_eq!(err, virtual_fs::FsError::AlreadyExists);
}

/// Cross-directory rename is not implemented yet.
#[tokio::test]
async fn test_rename_cross_dir_unsupported() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-rename".to_owned(), client);
    fs.create_dir(&PathBuf::from("/a")).unwrap();
    fs.create_dir(&PathBuf::from("/a/b")).unwrap();
    fs.create_dir(&PathBuf::from("/d")).unwrap();

    let err = fs
        .rename(&PathBuf::from("/a/b"), &PathBuf::from("/d/b"))
        .await
        .unwrap_err();
    assert_eq!(err, virtual_fs::FsError::Unsupported);
}
