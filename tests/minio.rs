/*!*
 * There are MiniO feature tests just to make sure our expectations are valid.
 */

use aws_sdk_s3::error::ProvideErrorMetadata as _;
use testcontainers::ContainerAsync;
use testcontainers_modules::{minio::MinIO, testcontainers::runners::AsyncRunner};

async fn minio_s3_client(container: &ContainerAsync<MinIO>) -> aws_sdk_s3::Client {
    let port = container.get_host_port_ipv4(9000).await.unwrap();
    let endpoint = format!("http://127.0.0.1:{port}");

    let config = aws_config::from_env()
        .endpoint_url(&endpoint)
        .credentials_provider(aws_credential_types::Credentials::new(
            "minioadmin",
            "minioadmin",
            None,
            None,
            "test",
        ))
        .region(aws_config::Region::new("us-east-1"))
        .load()
        .await;

    aws_sdk_s3::Client::new(&config)
}

#[tokio::test]
async fn test_cas_create() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "cas-create";
    let filename = "thefile";
    let _bucket = client
        .create_bucket()
        .bucket(bucket_name)
        .send()
        .await
        .unwrap();

    let body1 = aws_sdk_s3::primitives::ByteStream::from_static(b"body");
    let _upload1 = client
        .put_object()
        .bucket(bucket_name)
        .key(filename)
        .body(body1)
        .if_none_match("*")
        .send()
        .await
        .unwrap();

    let body2 = aws_sdk_s3::primitives::ByteStream::from_static(b"body");
    let upload_res2 = client
        .put_object()
        .bucket(bucket_name)
        .key(filename)
        .body(body2)
        .if_none_match("*")
        .send()
        .await;
    assert!(upload_res2.is_err(), "{upload_res2:?}");
    assert_eq!(upload_res2.unwrap_err().code(), Some("PreconditionFailed"));
}

#[tokio::test]
async fn test_cas_update() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "cas-update";
    let filename = "thefile";
    client
        .create_bucket()
        .bucket(bucket_name)
        .send()
        .await
        .unwrap();

    let body1 = aws_sdk_s3::primitives::ByteStream::from_static(b"body1");
    let body2 = aws_sdk_s3::primitives::ByteStream::from_static(b"body2");
    let body3 = aws_sdk_s3::primitives::ByteStream::from_static(b"body2");

    let _upload1 = client
        .put_object()
        .bucket(bucket_name)
        .key(filename)
        .body(body1)
        .send()
        .await
        .unwrap();

    let etag = client
        .head_object()
        .bucket(bucket_name)
        .key(filename)
        .send()
        .await
        .unwrap()
        .e_tag
        .unwrap();

    // Overwrite the object.
    let _upload2 = client
        .put_object()
        .bucket(bucket_name)
        .key(filename)
        .body(body2)
        .send()
        .await
        .unwrap();

    let upload_res3 = client
        .put_object()
        .bucket(bucket_name)
        .key(filename)
        .body(body3)
        .if_match(etag)
        .send()
        .await;
    
    assert!(upload_res3.is_err(), "{upload_res3:?}");
    assert_eq!(upload_res3.unwrap_err().code(), Some("PreconditionFailed"));
}
