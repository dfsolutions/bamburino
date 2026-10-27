# Self-hosted runner (on demand)

Some Bamburino checks (the Bambu Studio smoke test) open a GUI and need a real Windows desktop,
so they run on the maintainer's PC instead of GitHub-hosted runners.

## Why on demand, not a service

- The smoke test opens the Bambu Studio window: the runner must live in the maintainer's
  **interactive session**, so it is not installed with `--runasservice`.
- It does not start with Windows. A runner that is not running executes nothing, which keeps the
  exposure of the PC to the minimum: it is up only while a maintainer wants a build or a release check.
- Jobs queued while it is offline wait (up to GitHub's queue timeout) and run when it comes back.

## Identity

| Item | Value |
|---|---|
| Name | `federico-pc` |
| Labels | `self-hosted`, `Windows`, `X64`, `bamburino-win` |
| Runner version | 2.338.0 (self-updates when it connects) |
| Install dir | `D:\bb\runner` (outside every repository) |
| Work dir | `D:\bb\runner\_work` |

## Register, start, stop, remove

```powershell
pwsh ci/runner/register.ps1     # one time: download, verify SHA-256, register, secure the repo
pwsh ci/runner/start.ps1        # open the runner in its own window; "Listening for Jobs" = ready
```

Stop it by closing that window (or Ctrl+C). To remove it for good:

```powershell
$t = gh api -X POST repos/dfsolutions/bamburino/actions/runners/remove-token --jq .token
D:\bb\runner\config.cmd remove --token $t
```

`register.ps1` takes the GitHub token from Git Credential Manager (`tools/GitHub.psm1`,
`Set-GhToken`), checks the package SHA-256 against the value published in the release notes, and
never prints the registration token. The token is visible to local processes on the command line of
`config.cmd` for the few seconds the registration takes; it expires after one hour and is single use.

## What may run on it

Only workflows that pass `ci/check-workflows.ps1` (fail-closed static guard, enforced in CI):

- triggers are `push` to `main` or tags, never `pull_request`, `pull_request_target` or `workflow_run`;
- every job starts with the maintainer guard step (`ci/maintainers.txt`);
- explicit minimal `permissions`.

The static checker is a safety net, **not** the barrier for fork pull requests. The barrier is the
repository settings below, which keep untrusted code away from the runner.

## Repository settings

Applied and verified by `ci/runner/secure-repo.ps1` (also called by `register.ps1`), through the REST
API with the `repo`-scoped token. Result of the first run, 2026-10-27:

| Setting | Endpoint | Before | After | Result |
|---|---|---|---|---|
| Fork PR workflows: require approval for all external contributors | `PUT /repos/dfsolutions/bamburino/actions/permissions/fork-pr-contributor-approval` (`approval_policy=all_external_contributors`) | `first_time_contributors` | `all_external_contributors` | applied, verified by GET |
| Default workflow permissions `read`, Actions cannot approve PRs | `PUT /repos/dfsolutions/bamburino/actions/permissions/workflow` | `read`, `can_approve_pull_request_reviews=false` | unchanged | already set, verified by GET |

Both endpoints worked with a `repo`-scoped PAT. If a future run reports `forbidden-by-scope`, set
by hand in Settings > Actions > General: *Fork pull request workflows from outside collaborators* >
**Require approval for all external contributors**, and *Workflow permissions* > **Read repository
contents and packages permissions** with *Allow GitHub Actions to create and approve pull requests*
unticked. Re-run `pwsh ci/runner/secure-repo.ps1` to verify; it exits non-zero if anything differs.

## Tests

`pwsh ci/runner/tests/run.ps1` runs the unit tests (SHA-256 parsing, config arguments without
`--runasservice`, idempotent policy application against a fake API). They use no network.
