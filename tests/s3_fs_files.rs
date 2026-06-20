use std::path::PathBuf;

use s3::{Auth, BlockingClient, Credentials};
use s3_fs::S3FileSystem;
use testcontainers::ContainerAsync;
use testcontainers_modules::{minio::MinIO, testcontainers::runners::AsyncRunner};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use virtual_fs::{FileSystem, FsError};

/// Creates `path` with the given `contents` and closes it.
async fn write_file(fs: &S3FileSystem, path: &PathBuf, contents: &[u8]) {
    let mut file = fs
        .new_open_options()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    file.write_all(contents).await.unwrap();
    file.shutdown().await.unwrap();
}

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

/// Removing a file deletes its directory entry; the name is then free to reuse.
#[tokio::test]
async fn test_remove_file() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    let path = PathBuf::from("/removable");

    write_file(&fs, &path, b"to be deleted").await;
    assert!(fs.metadata(&path).is_ok());

    fs.remove_file(&path).unwrap();

    // The entry is gone: metadata and opening for read both fail.
    assert_eq!(fs.metadata(&path).unwrap_err(), FsError::EntryNotFound);
    fs.new_open_options().read(true).open(&path).unwrap_err();

    // And the name can be created anew.
    write_file(&fs, &path, b"recreated").await;
    let mut file = fs.new_open_options().read(true).open(&path).unwrap();
    let mut read_back = Vec::new();
    file.read_to_end(&mut read_back).await.unwrap();
    assert_eq!(read_back, b"recreated");
}

/// Removing a file inside a sub-directory works and leaves the directory intact.
#[tokio::test]
async fn test_remove_file_in_subdir() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    fs.create_dir(&PathBuf::from("/dir")).unwrap();

    let path = PathBuf::from("/dir/nested.txt");
    write_file(&fs, &path, b"nested").await;

    fs.remove_file(&path).unwrap();

    assert_eq!(fs.metadata(&path).unwrap_err(), FsError::EntryNotFound);
    // The parent directory itself still exists.
    assert!(fs.metadata(&PathBuf::from("/dir")).unwrap().is_dir());
}

/// `remove_dir` refuses to remove a regular file — even one whose contents
/// happen to look exactly like a (valid, empty) directory object. The rejection
/// must come from the entry's type, not from deserialization failing.
#[tokio::test]
async fn test_remove_dir_on_file_fails() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    let path = PathBuf::from("/afile");
    // Plant a file whose body is a perfectly valid serialized `DirObj`.
    write_file(&fs, &path, br#"{"children":{}}"#).await;

    assert_eq!(fs.remove_dir(&path).unwrap_err(), FsError::InvalidInput);
    // The file is untouched.
    assert!(fs.metadata(&path).unwrap().is_file());
}

/// Removing a file that does not exist fails with `EntryNotFound`.
#[tokio::test]
async fn test_remove_file_missing() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    assert_eq!(
        fs.remove_file(&PathBuf::from("/nope")).unwrap_err(),
        FsError::EntryNotFound
    );
}

/// `remove_file` refuses to remove a directory.
#[tokio::test]
async fn test_remove_file_on_dir_fails() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    let path = PathBuf::from("/adir");
    fs.create_dir(&path).unwrap();

    assert_eq!(fs.remove_file(&path).unwrap_err(), FsError::InvalidInput);
    // The directory is untouched.
    assert!(fs.metadata(&path).unwrap().is_dir());
}

/// A file can be renamed within its directory and read back under the new name.
#[tokio::test]
async fn test_rename_file_same_dir() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = S3FileSystem::init("fs-files".to_owned(), client);
    write_file(&fs, &PathBuf::from("/from.txt"), b"payload").await;

    fs.rename(&PathBuf::from("/from.txt"), &PathBuf::from("/to.txt"))
        .await
        .unwrap();

    fs.metadata(&PathBuf::from("/from.txt")).unwrap_err();

    let mut file = fs
        .new_open_options()
        .read(true)
        .open(&PathBuf::from("/to.txt"))
        .unwrap();
    let mut read_back = Vec::new();
    file.read_to_end(&mut read_back).await.unwrap();
    assert_eq!(read_back, b"payload");
}

/// Concurrently creating distinct files in the same parent must not lose
/// entries: each file's close registers into the parent via the shared CAS
/// loop. The old raw get+put registration could clobber a sibling's insert.
#[tokio::test]
async fn test_concurrent_file_create_same_dir() {
    use std::sync::Arc;

    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let fs = Arc::new(S3FileSystem::init("fs-files".to_owned(), client));
    fs.create_dir(&PathBuf::from("/p")).unwrap();

    const N: usize = 8;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let fs = Arc::clone(&fs);
            std::thread::spawn(move || {
                // The file I/O API is async but backed by the blocking client,
                // so a tiny current-thread runtime drives it to completion.
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async {
                    let mut f = fs
                        .new_open_options()
                        .write(true)
                        .create_new(true)
                        .open(&PathBuf::from(format!("/p/file{i}")))
                        .unwrap();
                    f.write_all(format!("body{i}").as_bytes()).await.unwrap();
                    f.shutdown().await.unwrap();
                });
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let count = fs.read_dir(&PathBuf::from("/p")).unwrap().count();
    assert_eq!(count, N);
}
