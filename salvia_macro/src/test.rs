#[test]
fn query_ui() {
    use trybuild::TestCases;

    let t = TestCases::new();

    t.pass("tests/query_ui/pass/*.rs");
    t.compile_fail("tests/query_ui/fail/*.rs");
}

#[test]
fn input_impl_ui() {
    use trybuild::TestCases;

    let t = TestCases::new();

    t.pass("tests/input_access_ui/pass/*.rs");
    t.compile_fail("tests/input_access_ui/fail/*.rs");
}
