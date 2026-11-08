//! Узел-телефон поднимается из пустой папки, отдаёт локальный API по токену и останавливается.

#[test]
fn phone_node_starts_and_serves_local_api() {
    let dir = std::env::temp_dir().join(format!("most-mobile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(most_mobile::start(dir.display().to_string(), "bad name!".into()).is_err());
    let s = most_mobile::start(dir.display().to_string(), "test-phone".into()).expect("start");
    assert!(s.url.starts_with("http://127.0.0.1:"), "{}", s.url);
    assert_eq!(s.node, "test-phone");
    assert_eq!(s.version, env!("CARGO_PKG_VERSION"));
    // повторный start — тот же узел
    assert_eq!(most_mobile::start(dir.display().to_string(), String::new()).unwrap().url, s.url);
    let cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("bridge.json")).unwrap()).unwrap();
    assert_eq!(cfg["turnPort"], 0);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let (st, body) = rt.block_on(async {
        let c = reqwest::Client::new();
        let r = c.get(format!("{}/bridge/v1/local/overview", s.url)).header("X-Bridge-Local", &s.token).send().await.unwrap();
        let st = r.status().as_u16();
        (st, r.text().await.unwrap())
    });
    assert_eq!(st, 200, "{}", body);
    let no = rt.block_on(async { reqwest::Client::new().get(format!("{}/bridge/v1/local/overview", s.url)).send().await.unwrap().status().as_u16() });
    assert_ne!(no, 200, "без токена API закрыт");

    most_mobile::network_changed();
    most_mobile::stop();
    assert!(most_mobile::status().is_none());
    // снова — из той же папки, имя уже в bridge.json
    let s2 = most_mobile::start(dir.display().to_string(), String::new()).expect("restart");
    assert_eq!(s2.node, "test-phone");
    most_mobile::stop();
}
