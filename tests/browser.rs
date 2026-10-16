use kern::drive::{browser_dockerfile, browser_need, lock_versions};

#[test]
fn the_playwright_version_is_read_from_every_kind_of_lockfile() {
    let npm = r#"{ "packages": { "node_modules/playwright-core": { "version": "1.49.1", "resolved": "https://registry.npmjs.org/playwright-core/-/playwright-core-1.49.1.tgz" } } }"#;
    assert_eq!(lock_versions("playwright-core", npm), [(1, 49, 1)]);
    let pnpm = "packages:\n\n  playwright-core@1.48.2:\n    resolution: {integrity: sha512-x}\n    engines: {node: '>=18'}\n";
    assert_eq!(lock_versions("playwright-core", pnpm), [(1, 48, 2)]);
    let yarn1 = "playwright-core@1.47.0, playwright-core@^1.47.0:\n  version \"1.47.0\"\n  resolved \"https://registry.yarnpkg.com/playwright-core/-/playwright-core-1.47.0.tgz\"\n";
    assert!(lock_versions("playwright-core", yarn1).contains(&(1, 47, 0)));
    let berry = "\"playwright-core@npm:1.50.0\":\n  version: 1.50.0\n  resolution: \"playwright-core@npm:1.50.0\"\n";
    assert!(lock_versions("playwright-core", berry).iter().all(|v| *v == (1, 50, 0)));
    assert!(lock_versions("playwright", "  @playwright/test@1.49.1:\n").is_empty(), "@playwright/test is not playwright");
    assert!(lock_versions("playwright", "  playwright-chromium@1.49.1:\n").is_empty());
}

#[test]
fn a_project_whose_tests_drive_a_browser_gets_one_and_the_rest_do_not() {
    let vitest = vec![r#"{"devDependencies":{"vitest":"3.2.4","@vitest/browser-playwright":"4.0.0"}}"#.to_string()];
    let lock = vec!["  playwright@1.56.1:\n    resolution: {integrity: x}\n  playwright-core@1.56.1:\n    resolution: {integrity: y}\n".to_string()];
    assert_eq!(browser_need(&vitest, &lock), Some(("1.56.1".to_string(), "the project uses @vitest/browser-playwright".to_string())), "vuetify, svelte, elevenlabs");
    let puppeteer = vec![r#"{"devDependencies":{"puppeteer":"24.1.0"}}"#.to_string()];
    assert_eq!(browser_need(&puppeteer, &[]).map(|(v, _)| v), Some(String::new()), "parse-dashboard and vue core use puppeteer, which brings its own Chrome");
    let e2e_only = vec![r#"{"devDependencies":{"@playwright/test":"1.49.1","vitest":"3.2.4"}}"#.to_string()];
    assert_eq!(browser_need(&e2e_only, &lock), None, "end-to-end tests that Kern does not run need no browser");
    assert_eq!(browser_need(&[r#"{"dependencies":{"react":"19.0.0"}}"#.to_string()], &[]), None);
}

#[test]
fn the_browser_image_is_the_projects_own_node_image_with_the_browser_its_playwright_expects() {
    let with = browser_dockerfile("kern-base:abc", "1.49.1");
    assert!(with.starts_with("FROM kern-base:abc\n") && with.contains("npx -y playwright@1.49.1 install --with-deps chromium") && with.contains("PLAYWRIGHT_BROWSERS_PATH=/ms-playwright"), "{with}");
    let deps = browser_dockerfile("kern-base:abc", "");
    assert!(deps.contains("install-deps chromium") && !deps.contains("install --with-deps"), "puppeteer downloads its own Chrome during install; the image only adds the system libraries: {deps}");
}
