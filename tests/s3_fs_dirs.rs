use std::path::PathBuf;

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
