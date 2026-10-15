use kern::policy::Runner;
use kern::script::{Found, found};
use serde_json::json;

fn run(scripts: serde_json::Value) -> (Runner, Vec<String>, Vec<(String, String)>) {
    match found(&json!({ "scripts": scripts })) {
        Found::Run(i) => (i.runner, i.command, i.env.into_iter().collect()),
        other => panic!("expected a runner, got {other:?}"),
    }
}
fn bin(name: &str) -> String { format!("/workspace/node_modules/.bin/{name}") }
fn words(list: &[&str]) -> Vec<String> { list.iter().map(|s| s.to_string()).collect() }
fn env(list: &[(&str, &str)]) -> Vec<(String, String)> { list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }

#[test]
fn a_test_script_that_runs_a_runner_keeps_its_own_arguments() {
    assert_eq!(run(json!({ "test": "jest --config .jest.js --no-cache" })), (Runner::Jest, words(&[&bin("jest"), "--config", ".jest.js", "--no-cache"]), vec![]));
    assert_eq!(run(json!({ "test": "TZ=UTC jest --config=.jestconfig.json" })), (Runner::Jest, words(&[&bin("jest"), "--config=.jestconfig.json"]), env(&[("TZ", "UTC")])));
    assert_eq!(run(json!({ "test": "mocha --exit --ignore \"test/encryption/**/*.test.js\" \"./test/**/*.test.js\"" })),
        (Runner::Mocha, words(&[&bin("mocha"), "--exit", "--ignore", "test/encryption/**/*.test.js", "./test/**/*.test.js"]), vec![]));
    assert_eq!(run(json!({ "test": "NODE_ENV=test mocha -r ts-node/register 'src/**/*.spec.ts' --reporter spec" })),
        (Runner::Mocha, words(&[&bin("mocha"), "-r", "ts-node/register", "src/**/*.spec.ts"]), env(&[("NODE_ENV", "test")])));
    assert_eq!(run(json!({ "test": "vitest --run" })), (Runner::Vitest, words(&[&bin("vitest"), "run"]), vec![]));
    assert_eq!(run(json!({ "test": "vitest" })), (Runner::Vitest, words(&[&bin("vitest"), "run"]), vec![]));
    assert_eq!(run(json!({ "test": "vitest watch --project unit" })), (Runner::Vitest, words(&[&bin("vitest"), "run", "--project", "unit"]), vec![]));
    assert_eq!(run(json!({ "test": "node --test test/" })), (Runner::Node, words(&["/usr/local/bin/node", "--test", "test/"]), vec![]));
    assert_eq!(run(json!({ "test": "node --import tsx --test 'test/**/*.test.ts'" })), (Runner::Node, words(&["/usr/local/bin/node", "--import", "tsx", "--test", "test/**/*.test.ts"]), vec![]));
    assert_eq!(run(json!({ "test": "tsx --test test/*.test.ts" })), (Runner::Node, words(&[&bin("tsx"), "--test", "test/*.test.ts"]), vec![]));
}

#[test]
fn wrappers_and_environment_are_read_the_way_a_shell_would() {
    assert_eq!(run(json!({ "test": "cross-env STRICT_MODE=1 VIRT_ON=1 yarn jest" })), (Runner::Jest, words(&[&bin("jest")]), env(&[("STRICT_MODE", "1"), ("VIRT_ON", "1")])));
    assert_eq!(run(json!({ "test": "NODE_OPTIONS=--max_old_space_size=12000 jest --maxWorkers=4" })),
        (Runner::Jest, words(&[&bin("jest"), "--maxWorkers=4"]), env(&[("NODE_OPTIONS", "--max_old_space_size=12000")])));
    assert_eq!(run(json!({ "test:unit": "cross-env NODE_OPTIONS='--experimental-vm-modules' NODE_NO_WARNINGS=1 jest --workerIdleMemoryLimit=1000MB" })),
        (Runner::Jest, words(&[&bin("jest"), "--workerIdleMemoryLimit=1000MB"]), env(&[("NODE_NO_WARNINGS", "1"), ("NODE_OPTIONS", "--experimental-vm-modules")])));
    assert_eq!(run(json!({ "test": "npx vitest run" })), (Runner::Vitest, words(&[&bin("vitest"), "run"]), vec![]));
    assert_eq!(run(json!({ "test": "pnpm exec jest" })), (Runner::Jest, words(&[&bin("jest")]), vec![]));
    assert_eq!(run(json!({ "test": "dotenv -e .env.test -- jest" })), (Runner::Jest, words(&[&bin("jest")]), vec![]));
    assert_eq!(run(json!({ "test": "c8 --reporter=lcov vitest run" })), (Runner::Vitest, words(&[&bin("vitest"), "run"]), vec![]));
    assert_eq!(run(json!({ "test": "nyc mocha" })), (Runner::Mocha, words(&[&bin("mocha")]), vec![]));
    assert_eq!(run(json!({ "test": "node --expose-gc --max-old-space-size=4096 --experimental-vm-modules node_modules/jest-cli/bin/jest --logHeapUsage" })),
        (Runner::Jest, words(&[&bin("jest"), "--logHeapUsage"]), env(&[("NODE_OPTIONS", "--expose-gc --max-old-space-size=4096 --experimental-vm-modules")])));
}

#[test]
fn a_script_that_names_another_script_is_followed_with_its_extra_arguments() {
    assert_eq!(run(json!({ "test": "pnpm test:unit && pnpm test:integration",
        "test:unit": "vitest run tests/unit --maxWorkers=2 --reporter=default --reporter=json --outputFile=tests/reports/unit.json", "test:integration": "vitest run tests/integration" })),
        (Runner::Vitest, words(&[&bin("vitest"), "run", "tests/unit", "--maxWorkers=2"]), vec![]));
    assert_eq!(run(json!({ "test": "yarn test:base --testMatch \"<rootDir>/test/unitCases/*.unittest.js\"", "test:base": "node --experimental-vm-modules node_modules/jest-cli/bin/jest" })),
        (Runner::Jest, words(&[&bin("jest"), "--testMatch", "<rootDir>/test/unitCases/*.unittest.js"]), env(&[("NODE_OPTIONS", "--experimental-vm-modules")])));
    assert_eq!(run(json!({ "test": "npm run unit -- --runInBand", "unit": "tsc -p . && jest" })), (Runner::Jest, words(&[&bin("jest"), "--runInBand"]), vec![]));
    assert_eq!(run(json!({ "test": "npm-run-all --npm-path npm -s lint jest test:peril", "lint": "eslint .", "jest": "jest", "test:peril": "jest -c .jestrc.peril.json" })),
        (Runner::Jest, words(&[&bin("jest")]), vec![]));
    assert_eq!(run(json!({ "test": "yarn vitest" })), (Runner::Vitest, words(&[&bin("vitest"), "run"]), vec![]));
    assert_eq!(found(&json!({ "scripts": { "test": "npm test" } })), Found::Nothing, "a script that names itself ends");
}

#[test]
fn a_script_that_hands_the_tests_to_each_package_says_so() {
    for script in ["dotenv -- turbo test", "cross-env CI=1 pnpm --recursive run test", "lerna run --concurrency 1 --stream test", "turbo run test",
        "pnpm nx run-many -t test --exclude @tryghost/e2e", "FORCE_COLOR=1 turbo test --concurrency=${TURBO_CONCURRENCY:-80%}", "yarn workspaces run test", "pnpm -r test"] {
        assert_eq!(found(&json!({ "scripts": { "test": script } })), Found::Packages, "{script}");
    }
    assert_eq!(found(&json!({ "scripts": { "test": "npm run cleanup && npm run test --workspaces --if-present", "cleanup": "rimraf dist" } })), Found::Packages);
    assert_eq!(found(&json!({ "scripts": { "test": "run-s common-test backend-test", "common-test": "pnpm -F common test", "backend-test": "pnpm -F backend test" } })), Found::Packages);
}

#[test]
fn a_script_kern_cannot_run_is_nothing_and_never_a_guess() {
    for script in ["node Makefile.js test", "echo 'Error: run a specific suite' && exit 1", "scripts/run-jest.sh --bundler=webpack", "vitest bench", "bun test",
        "echo Please run any of the test scripts from the scripts folder."] {
        assert_eq!(found(&json!({ "scripts": { "test": script } })), Found::Nothing, "{script}");
    }
    assert_eq!(found(&json!({ "scripts": {} })), Found::Nothing);
    assert_eq!(found(&json!({})), Found::Nothing);
}

#[test]
fn what_would_change_how_kern_reads_the_results_is_left_out() {
    assert_eq!(run(json!({ "test": "jest --ci --coverage --reporters=default --reporters=jest-junit --json --outputFile out.json --watchAll=false -u --bail" })),
        (Runner::Jest, words(&[&bin("jest"), "--ci"]), vec![]));
    assert_eq!(run(json!({ "test": "jest --notify --watch" })), (Runner::Jest, words(&[&bin("jest")]), vec![]));
    assert_eq!(run(json!({ "test": "jest --shard=${TEST_SHARD:-1}/${TEST_SHARD_TOTAL:-1} -w ${TEST_MAX_WORKERS:-100%} --runInBand" })),
        (Runner::Jest, words(&[&bin("jest"), "--runInBand"]), vec![]));
    assert_eq!(run(json!({ "test": "mocha -R dot --reporter-option maxDiffSize=0 -w test" })), (Runner::Mocha, words(&[&bin("mocha"), "test"]), vec![]));
    assert_eq!(run(json!({ "test": "vitest run --coverage.enabled --ui --reporter verbose -u" })), (Runner::Vitest, words(&[&bin("vitest"), "run"]), vec![]));
    assert_eq!(run(json!({ "test": "node --test --test-reporter=spec --experimental-test-coverage test/" })), (Runner::Node, words(&["/usr/local/bin/node", "--test", "test/"]), vec![]));
    assert_eq!(run(json!({ "test": "PATH=/evil:$PATH KERN_REPORT=/tmp/fake LD_PRELOAD=x.so HOME=/x CI=1 jest" })), (Runner::Jest, words(&[&bin("jest")]), env(&[("CI", "1")])),
        "variables that would steer Kern or the sandbox are left out");
}

#[test]
fn only_the_package_managers_own_options_decide_where_the_tests_run() {
    assert_eq!(run(json!({ "test": "npm run jest -- -w 4", "jest": "jest" })), (Runner::Jest, words(&[&bin("jest"), "-w", "4"]), vec![]), "-w after -- is jest's, not npm's");
    assert_eq!(run(json!({ "test": "npm --loglevel silent run unit", "unit": "jest" })), (Runner::Jest, words(&[&bin("jest")]), vec![]));
    assert_eq!(run(json!({ "test": "yarn unit -r ./setup.js", "unit": "mocha" })), (Runner::Mocha, words(&[&bin("mocha"), "-r", "./setup.js"]), vec![]));
    for script in ["pnpm -C packages/web test", "yarn --cwd web test", "npm --prefix web test", "pnpm run -r test", "npm run test --workspaces", "pnpm --filter=web test"] {
        assert_eq!(found(&json!({ "scripts": { "test": script } })), Found::Packages, "{script}");
    }
}
