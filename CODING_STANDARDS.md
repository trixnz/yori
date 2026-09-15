# Coding standards

## Test value

A regression test must exercise a plausible failure mechanism and assert an observable behavior or invariant.

During review, reject tests that only:

- copy a production constant into an equality assertion;
- restate the structure of the implementation;
- prove that unconditionally deleted code is absent;
- pin incidental pixel values when no behavior depends on the exact value.

Geometry tests are valuable when they assert relationships that can break independently, such as two regions meeting without a gap, a target remaining inside its container, or computed layout responding correctly to resize. Prefer those invariants over duplicated coordinates.

A test name or failure message should make the guarded defect clear. If removing the test would leave no realistic regression undetected, remove the test.
