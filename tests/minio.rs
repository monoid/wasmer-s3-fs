/*!*
 * There are MiniO feature tests just to make sure our expectations are valid.
 */

use s3::{Auth, BlockingClient, Credentials};
use testcontainers::ContainerAsync;
use testcontainers_modules::{minio::MinIO, testcontainers::runners::AsyncRunner};

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
async fn test_cas_create() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "cas-create";
    let filename = "thefile";

    client.buckets().create(bucket_name).send().unwrap();

    client
        .objects()
        .put(bucket_name, filename)
        .body_bytes(b"body1".to_vec())
        .if_none_match("*")
        .unwrap()
        .send()
        .unwrap();

    let err = client
        .objects()
        .put(bucket_name, filename)
        .body_bytes(b"body2".to_vec())
        .if_none_match("*")
        .unwrap()
        .send()
        .unwrap_err();

    assert!(
        matches!(
            &err,
            s3::Error::Api { status, code, .. }
                if status.as_u16() == 412
                && code.as_deref() == Some("PreconditionFailed")
        ),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_cas_update() {
    let container = MinIO::default().start().await.unwrap();
    let client = minio_s3_client(&container).await;

    let bucket_name = "cas-update";
    let filename = "thefile";
    client.buckets().create(bucket_name).send().unwrap();

    client
        .objects()
        .put(bucket_name, filename)
        .body_bytes(b"body1".to_vec())
        .send()
        .unwrap();

    let etag = client
        .objects()
        .head(bucket_name, filename)
        .send()
        .unwrap()
        .etag
        .unwrap();

    client
        .objects()
        .put(bucket_name, filename)
        .body_bytes(b"body2".to_vec())
        .send()
        .unwrap();

    let err = client
        .objects()
        .put(bucket_name, filename)
        .body_bytes(b"body2".to_vec())
        .if_match(&etag)
        .unwrap()
        .send()
        .unwrap_err();

    assert!(
        matches!(
            &err,
            s3::Error::Api { status, code, .. }
                if status.as_u16() == 412
                && code.as_deref() == Some("PreconditionFailed")
        ),
        "unexpected error: {err:?}"
    );
}
