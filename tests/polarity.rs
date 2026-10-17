use kern::polarity::{Kind, read};

fn kind(message: &str, expected: Option<&str>, actual: Option<&str>) -> Kind { read(message, expected, actual).kind }
fn check(cases: &[(&str, &str, Option<&str>, Option<&str>)], want: Kind) {
    for (pr, message, expected, actual) in cases {
        let r = read(message, *expected, *actual);
        assert_eq!(r.kind, want, "{pr}: {message:?} -> {r:?}");
    }
}

#[test]
fn vuejs_core_15867_an_old_test_that_expected_a_refusal_the_new_code_accepts_is_opened() {
    check(&[
        ("vuejs/core#15867", "AssertionError: expected [Function] to throw an error", Some("undefined"), Some("undefined")),
        ("yamcodes/arkenv#1507", "AssertionError: expected [Function] to throw an error", None, None),
        ("jest toThrow", "Error: expect(received).toThrow()\n\nReceived function did not throw", None, None),
        ("jest rejects", "Error: expect(received).rejects.toThrow()\n\nReceived promise resolved instead of rejected\nResolved to value: {\"ok\": true}", None, None),
        ("vitest rejects", "AssertionError: promise resolved \"{ ok: true }\" instead of rejecting", None, None),
        ("node assert.throws", "AssertionError [ERR_ASSERTION]: Missing expected exception.", None, None),
        ("lobehub/lobehub#20981", "AssertionError: expected \"vi.fn()\" to not be called at all, but actually been called 1 times\n\nReceived:\n\n  1st vi.fn() call:\n\n    Array [\n      Object {\n        \"fileId\": \"file_1\",\n      },\n    ]\n\n\nNumber of calls: 1\n", None, None),
        ("jest not.toHaveBeenCalled", "Error: expect(jest.fn()).not.toHaveBeenCalled()\n\nExpected number of calls: 0\nReceived number of calls: 1\n\n1: \"x\"", None, None),
        ("adobe/da-admin#271", "must return 500 when version was not created", Some("500"), Some("201")),
        ("jest status", "Error: expect(received).toBe(expected) // Object.is equality\n\nExpected: 403\nReceived: 200", None, None),
    ], Kind::Opened);
    let r = read("must return 500 when version was not created", Some("500"), Some("201"));
    assert_eq!((r.was.as_str(), r.now.as_str()), ("500", "201"));
}

#[test]
fn harperfast_create_harper_123_a_yes_or_no_a_something_or_nothing_or_a_new_shape_is_flipped() {
    check(&[
        ("HarperFast/create-harper#123", "AssertionError: expected true to be false // Object.is equality", None, None),
        ("QAInsights#34", "AssertionError: expected false to be true // Object.is equality", None, None),
        ("sveltejs/kit#14131", "AssertionError: expected undefined to equal true", None, None),
        ("WYRE-AI#114", "AssertionError: expected undefined to be false // Object.is equality", None, None),
        ("DevCycleHQ/js-sdks#1082", "Error: expect(received).toBeNull()\n\nReceived: \"\\\"anon_user_id\\\"\"", None, None),
        ("workos/authkit-react-router#78", "Error: expect(received).toEqual(expected) // deep equality\n\nExpected: Any<String>\nReceived: undefined", None, None),
        ("adobe/aio-cli-plugin-runtime#413", "Error: expect(jest.fn()).toHaveBeenNthCalledWith(n, ...expected)\n\nn: 1\nExpected: [{\"choices\": [{\"name\": \"Destination\", \"value\": \"destination\"}], \"name\": \"type\"}]\n\nNumber of calls: 0", None, None),
        ("adobe/da-admin#271", "500 == 201", Some("201"), Some("500")),
        ("F1-fantazy-bot#73", "Error: expect(received).toEqual(expected) // deep equality\n\n- Expected  - 0\n+ Received  + 2\n\n  Object {\n    \"2025-05-25T13:00:00.000Z\": Object {\n+     \"cloudCover\": null,\n+     \"humidity\": null,\n      \"precipitation\": null,\n    },\n  }", None, None),
        ("dquancruz/finance-tracker#36", "Error: expect(jest.fn()).toHaveBeenCalledWith(...expected)\n\n- Expected\n+ Received\n\n  Object {\n    \"email\": \"person@example.com\",\n+   \"role\": \"user\",\n    \"sub\": \"user-1\",\n  },\n\nNumber of calls: 1", None, None),
        ("agentforscience/flamebird#13", "expected 'undefined' to be 'function' // Object.is equality", None, None),
        ("equinor/design-system#4134", "AssertionError: expected '' to be 'auto' // Object.is equality", None, None),
        ("resend/react-email#3120", "AssertionError: expected { body: { fontSize: 'NaNem' } } to deeply equal { body: { fontSize: '2em' } }", None, None),
        ("pmndrs/timeline#8", "AssertionError: expected [ +0, +0 ] to deeply equal [ 1, +0 ]", None, None),
        ("descriptinc/dependicus#63", "AssertionError: expected [ { number: 43, …(6) } ] to have a length of +0 but got 1", None, None),
        ("HarperFast/create-harper#123 list", "AssertionError: expected [ 'vanilla', 'react', 'vue', 'nextjs' ] to deeply equal [ 'vanilla', 'react', 'vue' ]", None, None),
        ("epam/ketcher#8131", "Error: expect(received).toMatchSnapshot()\n\nSnapshot name: `Save Dialog 1`\n\n- Snapshot  - 3\n+ Received  + 3\n\n@@ -118,18 +118,18 @@\n                  <li\n                    class=\"tabs\"\n                  >\n-                   <a\n+                   <button\n                      class=\"active\"\n-                     role=\"button\"\n                      tabindex=\"0\"\n+                     type=\"button\"\n                    >", None, None),
        ("InsForge-sdk-js#121", "expected { data: null, …(1) } to deeply equal { data: { ok: 1 }, error: null }",
            Some("Object {\n  \"data\": Object {\n    \"ok\": 1,\n  },\n  \"error\": null,\n}"), Some("Object {\n  \"data\": null,\n  \"error\": Object {\n    \"message\": \"boom\",\n  },\n}")),
        ("openai/codex-action#91", "AssertionError [ERR_ASSERTION]: Actor 'dependabot[bot]' is not permitted\n\n1 !== 0\n", None, None),
        ("formbricks/formbricks#8269", "AssertionError: expected \"vi.fn()\" to be called with arguments: [ Array(1) ]\n\nReceived:\n\n  1st vi.fn() call:\n\n  [\n    {\n+     \"omit\": {\n+       \"secret\": true,\n+     },\n      \"where\": {\n        \"id\": \"test-webhook-id\",\n      },\n    },\n  ]\n\n\nNumber of calls: 1\n", None, None),
        ("jest calls stopped", "Error: expect(jest.fn()).toHaveBeenCalledTimes(expected)\n\nExpected number of calls: 2\nReceived number of calls: 0", None, None),
    ], Kind::Flipped);
}

#[test]
fn calcom_28622_a_plain_string_or_number_that_moved_is_changed_with_both_values() {
    check(&[
        ("Comfy-Org/comfy-typescript-sdk#110", "AssertionError: expected '/v2/models/fal-ai/flux-pro' to be '/v1/models/fal-ai/flux-pro' // Object.is equality", None, None),
        ("adobe/rum-distiller#56", "AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\n+ actual - expected\n\n+ 'earned:ai:chatgpt'\n- 'owned:ai:chatgpt'", None, None),
        ("nearform/fast-jwt#652", "AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:\n+ actual - expected\n\n  Comparison {\n+   message: 'The cacheTTL option must be a non-negative number.'\n-   message: 'The cacheTTL option must be a finite, non-negative number.'\n  }\n", None, None),
        ("keycardai/typescript-sdk#248", "Error: expect(received).toEqual(expected) // deep equality\n\n- Expected  - 2\n+ Received  + 2\n\n@@ -5,9 +5,9 @@\n      \"clientId\": \"web-client\",\n-     \"redirectUri\": \"https://agent.example.com/connections/calendar/callback\",\n-     \"state\": \"state-1\",\n+     \"redirectUri\": \"https://agent.example.com/keycard/callback\",\n+     \"state\": \"state-1.aHR0cHM6Ly9hZ2VudC5leGFtcGxl\",\n    },\n  ]", None, None),
        ("equinor/design-system#4134 snapshot", "Error: expect(received).toMatchSnapshot()\n\nSnapshot name: `Accordion Matches snapshot 1`\n\n- Snapshot  - 2\n+ Received  + 2\n\n@@ -96,11 +96,11 @@\n          <button\n-           aria-controls=\"eds-accordion-1-panel-1\"\n+           aria-controls=\"test-id-panel-1\"\n            aria-expanded=\"true\"\n        <div\n-         aria-labelledby=\"eds-accordion-1-header-1\"\n+         aria-labelledby=\"test-id-header-1\"\n          class=\"c5\"", None, None),
        ("facebook/stylex#1888 snapshot", "Error: expect(received).toMatchInlineSnapshot(snapshot)\n\nSnapshot name: `x 3`\n\n- Snapshot  - 1\n+ Received  + 1\n\n  \"import * as stylex from '@stylexjs/stylex';\n  export const styles = {\n    root: {\n-     kWkggS: \"xrkmrrc x1qc147k x9qmkci x17z8iku\",\n+     kWkggS: \"xrkmrrc xa5oah1 x1seukri xduqtph\",\n      $$css: true\n    }\n  };\"", None, None),
        ("facebook/stylex#1888 toBe", "Error: expect(received).toBe(expected) // Object.is equality\n\nExpected: \"{\\\"width\\\":\\\"(screen)\\\"}\"\nReceived: \"{\\\"width\\\":\\\"screen\\\"}\"", None, None),
        ("scalar/scalar#9334", "AssertionError: expected 61 to be 58 // Object.is equality", None, None),
        ("agentforscience/flamebird#13", "expected 12 to be 6 // Object.is equality", None, None),
        ("netlify/build#6670", "AssertionError: expected 'nuxt build' to be 'nuxt generate' // Object.is equality", None, None),
        ("ocean-ds/ocean-web#1278", "Error: expect(element).toHaveClass(\"ods-transaction-footer-v2__row-label\")\n\nExpected the element to have class:\n  ods-transaction-footer-v2__row-label\nReceived:\n  ods-typography ods-content-list__text", None, None),
        ("calcom/cal.diy#28622", "AssertionError: expected { isValid: false, …(1) } to deeply equal { isValid: false, …(1) }",
            Some("Object {\n  \"error\": \"Private IP address\",\n  \"isValid\": false,\n}"), Some("Object {\n  \"error\": \"Blocked hostname\",\n  \"isValid\": false,\n}")),
        ("DevCycleHQ/js-sdks#1082 calls", "Error: expect(jest.fn()).toHaveBeenCalledTimes(expected)\n\nExpected number of calls: 1\nReceived number of calls: 3", None, None),
        ("vitest throw message", "AssertionError: expected [Function] to throw error including 'Invalid URL' but got 'Blocked hostname'", None, None),
        ("webdriverio#15609", "AssertionError: expected 'Timeout after 197ms in \"fn\" (WDIO att…' to deeply equal 'Timeout'", Some("Timeout"), Some("Timeout after 197ms in \"fn\" (WDIO attempt 2)")),
    ], Kind::Changed);
    let r = read("AssertionError: expected { isValid: false, …(1) } to deeply equal { isValid: false, …(1) }",
        Some("Object {\n  \"error\": \"Private IP address\",\n  \"isValid\": false,\n}"), Some("Object {\n  \"error\": \"Blocked hostname\",\n  \"isValid\": false,\n}"));
    assert_eq!((r.was.as_str(), r.now.as_str()), ("\"error\": \"Private IP address\",", "\"error\": \"Blocked hostname\","));
    let r = read("AssertionError: expected '/v2/models/fal-ai/flux-pro' to be '/v1/models/fal-ai/flux-pro' // Object.is equality", None, None);
    assert_eq!((r.was.as_str(), r.now.as_str()), ("'/v1/models/fal-ai/flux-pro'", "'/v2/models/fal-ai/flux-pro'"));
    let r = read("AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\n+ actual - expected\n\n+ 'earned:ai:chatgpt'\n- 'owned:ai:chatgpt'", None, None);
    assert_eq!((r.was.as_str(), r.now.as_str()), ("'owned:ai:chatgpt'", "'earned:ai:chatgpt'"));
    let r = read("AssertionError: expected 61 to be 58 // Object.is equality", None, None);
    assert_eq!((r.was.as_str(), r.now.as_str()), ("58", "61"));
}

#[test]
fn budibase_18627_an_old_test_that_cannot_drive_the_new_code_is_harness() {
    check(&[
        ("Budibase#18627", "Error: [vitest] No \"automationStore\" export is defined on the \"@/stores/builder\" mock. Did you forget to return it from \"vi.mock\"?", None, None),
        ("Cap-go/electron-updater#7", "this.storage.getDelayConditionMode is not a function", None, None),
        ("NanHeSam#2", "Error: Expected a Convex function exported from module \"billing\" as `createAlphaSubscription`, but there is no such export.", None, None),
        ("KentekenBot#136", "TypeError: Cannot read properties of undefined (reading 'id')", None, None),
        ("pmndrs/timeline#8", "TypeError: __vite_ssr_import_2__.SynchronousAbortController is not a constructor", None, None),
        ("godaddy/gasket#1150", "jest is not defined", None, None),
        ("lobehub#20981", "Error: Seems like you have not used zustand provider as an ancestor.", None, None),
    ], Kind::Harness);
}

#[test]
fn hochfrequenz_ahb_tabellen_932_a_failure_kern_cannot_read_stays_unknown() {
    check(&[
        ("Hochfrequenz/ahb-tabellen#932", "Error: expect(jest.fn()).toHaveBeenCalledWith(...expected)\n\nExpected: StringContaining \"REPLACE(LOWER(al.\", ObjectContaining {\"q0\": \"%foo\"}\nReceived\n       1: \"(REPLACE(LOWER(al.format_version) LIKE :q0\", {\"q0\": \"%foo%\"}", None, None),
        ("autoguru-au/overdrive#1167", "Error: Snapshot `<Tolltip /> > should match snapshot when closed 1` mismatched", None, None),
        ("sanity-io/client#1219 timeout", "Error: Test timed out in 5000ms.\nIf this is a long-running test, pass a timeout value", None, None),
        ("calcom/cal.diy#28622 cut short", "AssertionError: expected { isValid: false, …(1) } to deeply equal { isValid: false, …(1) }", None, None),
        ("sanity-io/client#1219 instance", "AssertionError: expected { type: 'reconnect' } to be an instance of CorsOriginError", None, None),
        ("a crash with only a stack", "Error: boom\n    at foo (/workspace/a.js:1:1)", None, None),
    ], Kind::Unknown);
}

#[test]
fn colours_in_a_message_change_nothing_and_no_input_can_crash_the_reader() {
    let coloured = "AssertionError: expected \u{1b}[31m'/v2/models/fal-ai/flux-pro'\u{1b}[39m to be \u{1b}[32m'/v1/models/fal-ai/flux-pro'\u{1b}[39m // Object.is equality";
    assert_eq!(kind(coloured, None, None), Kind::Changed);
    let samples = ["Error: expect(received).toEqual(expected) // deep equality\n\n- Expected  - 0\n+ Received  + 2\n\n  Object {\n+     \"cloudCover\": null,\n  }",
        "Error: expect(received).toEqual(expected)\n\n- Expected  - 2\n+ Received  + 2\n\n@@ -5,9 +5,9 @@\n-     \"redirectUri\": \"https://a\",\n+     \"redirectUri\": \"https://b\","];
    for s in samples { for end in (0..s.len()).filter(|e| s.is_char_boundary(*e)) { let _ = read(&s[..end], None, None); } }
    for odd in ["", "\"", "'", "`", "- ", "+", "expected  to be ", "Expected:\nReceived:", "1.2.3 == 1.2.4", "\u{1b}[", "+0 !== -"] { let _ = read(odd, Some(odd), Some(odd)); }
    let many = format!("- Expected\n+ Received\n{}", "-\n".repeat(10_000));
    let _ = read(&many, None, None);
    let wide = format!("Object {{\n{}}}", "  \"k\": 1,\n".repeat(5_000));
    assert_eq!(kind("x", Some(&wide), Some(&wide.replace('1', "2"))), Kind::Unknown, "a value too long to diff is not read");
}
