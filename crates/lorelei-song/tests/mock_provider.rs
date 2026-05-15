use lorelei_core::SongRequest;
use lorelei_core::SongProvider;
use lorelei_song::MockProvider;

#[tokio::test]
async fn mock_provider_works() {
    let p = MockProvider {
        response: "hi".to_string().into(),
    };

    let r = p
        .song(SongRequest {
            prompt: "x".to_string(),
            context: vec![],
            max_chunks: None,
            parameters: Default::default(),
        })
        .await
        .unwrap();

    assert_eq!(r.chunks[0].content, "hi");
}
