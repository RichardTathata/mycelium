# Sourced by every bash step in CI (BASH_ENV, .github/workflows/ci.yml): stderr joins stdout on one pipe.
# The runner reads the two separately, so their order in the log was not kept — cargo's "Running <target>"
# headers (stderr) drifted away from the test results (stdout) they introduce, and the test-coverage job,
# which attributes each result to the header above it, filed tests under the wrong target. On one pipe,
# cargo's header and its child's output are written in order.
exec 2>&1
