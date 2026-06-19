use std::path::PathBuf;

use s3::{Auth, BlockingClient, Credentials};
use s3_fs::S3FileSystem;
use testcontainers::ContainerAsync;
use testcontainers_modules::{minio::MinIO, testcontainers::runners::AsyncRunner};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use virtual_fs::FileSystem;

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

/// Writes a new file, then reads it back through a fresh handle.
#[tokio::test]
async fn test_write_then_read() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    let path = PathBuf::from("/hello.txt");
    let contents = b"hello, s3 virtual file!";

    let mut file = fs
        .new_open_options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(contents).await.unwrap();
    file.shutdown().await.unwrap();
    drop(file);

    // The directory entry should now exist and report the right size.
    let metadata = fs.metadata(&path).unwrap();
    assert!(metadata.is_file());
    assert_eq!(metadata.len, contents.len() as u64);

    let mut file = fs.new_open_options().read(true).open(&path).unwrap();
    let mut read_back = Vec::new();
    file.read_to_end(&mut read_back).await.unwrap();
    assert_eq!(read_back, contents);
}

/// An empty file round-trips (the multipart upload is replaced by an empty PUT).
#[tokio::test]
async fn test_write_empty_file() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    let path = PathBuf::from("/empty");

    let mut file = fs
        .new_open_options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.shutdown().await.unwrap();
    drop(file);

    assert_eq!(fs.metadata(&path).unwrap().len, 0);

    let mut file = fs.new_open_options().read(true).open(&path).unwrap();
    let mut read_back = Vec::new();
    file.read_to_end(&mut read_back).await.unwrap();
    assert!(read_back.is_empty());
}

/// Seeking is supported for reads.
#[tokio::test]
async fn test_read_with_seek() {
    use std::io::SeekFrom;

    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    let path = PathBuf::from("/seekable");
    let contents = b"0123456789";

    let mut file = fs
        .new_open_options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(contents).await.unwrap();
    file.shutdown().await.unwrap();
    drop(file);

    let mut file = fs.new_open_options().read(true).open(&path).unwrap();

    file.seek(SeekFrom::Start(4)).await.unwrap();
    let mut buf = [0u8; 3];
    file.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"456");

    file.seek(SeekFrom::End(-2)).await.unwrap();
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).await.unwrap();
    assert_eq!(tail, b"89");
}

/// A file inside a sub-directory can be created and read.
#[tokio::test]
async fn test_write_in_subdir() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    fs.create_dir(&PathBuf::from("/dir")).unwrap();

    let path = PathBuf::from("/dir/nested.txt");
    let mut file = fs
        .new_open_options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(b"nested").await.unwrap();
    file.shutdown().await.unwrap();
    drop(file);

    let mut file = fs.new_open_options().read(true).open(&path).unwrap();
    let mut read_back = Vec::new();
    file.read_to_end(&mut read_back).await.unwrap();
    assert_eq!(read_back, b"nested");
}

/// Re-creating an existing file is rejected (no in-place updates).
#[tokio::test]
async fn test_create_existing_fails() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    let path = PathBuf::from("/once");

    let mut file = fs
        .new_open_options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(b"first").await.unwrap();
    file.shutdown().await.unwrap();
    drop(file);

    fs.new_open_options()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap_err();
}

/// Reading a file that does not exist fails.
#[tokio::test]
async fn test_read_missing_fails() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    fs.new_open_options()
        .read(true)
        .open(&PathBuf::from("/nope"))
        .unwrap_err();
}

/// Unsupported open flag combinations (here: append) are rejected.
#[tokio::test]
async fn test_append_unsupported() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    fs.new_open_options()
        .write(true)
        .append(true)
        .create(true)
        .open(&PathBuf::from("/appended"))
        .unwrap_err();
}
