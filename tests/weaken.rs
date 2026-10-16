use kern::weaken::{Case, cases, compare};
use std::collections::BTreeSet;

fn both(old: &str, new: &str) -> kern::weaken::Weakened {
    let (a, b) = (cases("test/run.test.ts", old).unwrap(), cases("test/run.test.ts", new).unwrap());
    compare(&a, &b, &BTreeSet::new())
}

#[test]
fn every_test_and_its_checks_are_listed_under_its_full_name() {
    let source = "import { describe, it, test, expect } from 'vitest';\nimport assert from 'node:assert';\n\
        describe('cancel', () => {\n  it('sweeps running entries', () => { expect(run()).toBe(1); expect(log).toHaveLength(2); });\n  \
        describe('errors', () => { test('marks failed', async () => { await expect(p).rejects.toThrow('x'); }); });\n});\n\
        test.each([[1, 2]])('adds %i', (a, b) => { assert.equal(a + 1, b); });\n\
        it('is there', () => { expect(x).toBeDefined(); assert(y); });\n\
        it('uses a helper', () => { expectValid(x); });\n";
    let found = cases("test/run.test.ts", source).unwrap();
    assert_eq!(found["cancel > sweeps running entries"], Case { checks: 2, weak: 0 });
    assert_eq!(found["cancel > errors > marks failed"], Case { checks: 1, weak: 0 });
    assert_eq!(found["adds %i"], Case { checks: 1, weak: 0 });
    assert_eq!(found["is there"], Case { checks: 2, weak: 2 });
    assert_eq!(found["uses a helper"], Case { checks: 1, weak: 0 }, "a helper named like a check counts as one");
    assert_eq!(found.len(), 5);
}

#[test]
fn tests_deleted_from_an_existing_file_are_named() {
    let old = "describe('cancel', () => {\n  it('cancels running entries before adding the synthetic entry', () => { expect(a).toBe(1); });\n  \
        it('reconciles final block logs', () => { expect(b).toBe(2); });\n  it('keeps going', () => { expect(c).toBe(3); });\n});\n";
    let new = "describe('cancel', () => {\n  it('keeps going', () => { expect(c).toBe(3); });\n});\n";
    let w = both(old, new);
    assert_eq!(w.removed, 2, "sim#4382 deleted the tests that checked the cancel sweep, then the sweep");
    assert_eq!(w.missing, ["cancel > cancels running entries before adding the synthetic entry", "cancel > reconciles final block logs"]);
}

#[test]
fn a_renamed_test_is_not_a_deleted_one_and_a_moved_test_is_not_either() {
    let old = "it('adds', () => { expect(add(1, 2)).toBe(3); });\nit('subtracts', () => { expect(sub(3, 1)).toBe(2); });\n";
    let new = "it('adds two numbers', () => { expect(add(1, 2)).toBe(3); });\nit('subtracts', () => { expect(sub(3, 1)).toBe(2); });\n";
    assert_eq!(both(old, new).removed, 0);
    let a = cases("test/a.test.ts", old).unwrap();
    let b = cases("test/a.test.ts", "it('subtracts', () => { expect(sub(3, 1)).toBe(2); });\n").unwrap();
    let elsewhere: BTreeSet<String> = ["adds".to_string()].into_iter().collect();
    assert_eq!(compare(&a, &b, &elsewhere).removed, 0, "a test the change moves to another test file is still there");
}

#[test]
fn a_test_left_checking_nothing_and_a_check_loosened_to_almost_nothing_are_named() {
    let old = "it('totals', () => { expect(total(items)).toBe(42); });\nit('discounts', () => { expect(discount(100)).toBe(90); });\n";
    let new = "it('totals', () => { total(items); });\nit('discounts', () => { expect(discount(100)).toBeDefined(); });\n";
    let w = both(old, new);
    assert_eq!(w.emptied, ["totals"]);
    assert_eq!(w.loosened, ["discounts"]);
    assert_eq!(w.removed, 0);
}

#[test]
fn ordinary_edits_to_tests_are_not_weakening() {
    let old = "it('totals', () => { expect(total(items)).toBe(42); });\n";
    let more = "it('totals', () => { expect(total(items)).toBe(43); expect(total([])).toBe(0); });\nit('new case', () => { expect(x).toEqual({ a: 1 }); });\n";
    assert_eq!(both(old, more), kern::weaken::Weakened::default(), "a changed expected value is a rewrite, which the test runs judge; more tests and checks are not weaker");
    let helper = "it('totals', () => { checkTotal(items, 42); });\n";
    assert_eq!(both(old, helper), kern::weaken::Weakened::default(), "a check moved into a helper still checks");
}
