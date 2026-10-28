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
| Install dir | `D:\BambuBuild\runner` (outside every repository) |
| Work dir | `D:\BambuBuild\runner\_work` |

The work dir is also stored as an absolute path in `D:\BambuBuild\runner\.runner` (`workFolder`, backslashes doubled in the JSON). If the build folder is ever moved or renamed, repeat the step: stop the runner, move the folder, edit `workFolder` in `.runner` to the new path (keep the JSON valid; the file is hidden, clear the attribute before saving), then start it. The folder was renamed from `D:\bb` on 2026-10-28.

## Register, start, stop, remove

```powershell
pwsh ci/runner/register.ps1     # one time: download, verify SHA-256, register, secure the repo
pwsh ci/runner/start.ps1        # open the runner in its own window; "Listening for Jobs" = ready
```

### Stop

```powershell
pwsh ci/runner/stop.ps1
```

`start.ps1` records the launched process id in `D:\BambuBuild\runner\.bamburino-runner.pid`. `stop.ps1`
asks that process tree to exit, waits up to 20 s, then force-stops any remaining `Runner.Listener` /
`Runner.Worker`, waits until none is left, and finally polls
`gh api repos/dfsolutions/bamburino/actions/runners` (up to about 90 s) until `federico-pc` is
`offline`. It exits 0 only when both conditions hold, non-zero otherwise.

Closing the window with the X, or killing the processes by hand, leaves the runner shown as
`online` on GitHub for about a minute; a job queued in that window is assigned to a runner that is
gone and stalls. Use `stop.ps1` and trust its exit code.

Details worth knowing:

- The pid file holds the pid, the process start time and the `run.cmd` path. `stop.ps1` acts on
  that pid only if the process is still `cmd.exe`, started within a second of the recorded time and
  running that `run.cmd`; otherwise it logs `stale pid file, ignored`, deletes the file and uses the
  name-based fallback (a pid can be reassigned after a reboot or a window close).
- The name-based fallback stops **any** `Runner.Listener` / `Runner.Worker` process on the machine,
  not only the one started by `start.ps1`.
- A runner that GitHub no longer lists (`absent`, for example after `config.cmd remove`) counts as
  success, the same as `offline`.

### Remove

To remove it for good:

```powershell
$t = gh api -X POST repos/dfsolutions/bamburino/actions/runners/remove-token --jq .token
D:\BambuBuild\runner\config.cmd remove --token $t
```

Notes on `register.ps1`:

- Do **not** run it under `Start-Transcript` (or any other logging of the console/command line): the
  one-time registration token is on the `config.cmd` command line. If `config.cmd` fails, the script
  reports only `config.cmd failed with exit <code>` and the output with the token masked.
- If `D:\BambuBuild\runner\config.cmd` already exists, the download and SHA-256 check are skipped (only the
  registration is redone, `--replace`). Delete the directory to force a fresh download.

`register.ps1` takes the GitHub token from Git Credential Manager (`tools/GitHub.psm1`,
`Set-GhToken`), checks the package SHA-256 against the value published in the release notes, and
never prints the registration token. The token is visible to local processes on the command line of
`config.cmd` for the few seconds the registration takes; it expires after one hour and is single use.

## What may run on it

Only workflows that pass `ci/check-workflows.ps1` (fail-closed static guard, enforced in CI):

- triggers are `push` to `main` or tags, never `pull_request`, `pull_request_target` or `workflow_run`;
- a separate `guard` job gates every self-hosted job via `needs` (maintainer check against `ci/maintainers.txt`);
- explicit minimal `permissions`.

A job on the self-hosted runner runs with the full rights of the owner's Windows user: the Git
Credential Manager PAT, SSH keys and open vault sessions are all reachable from a workflow step.
Hardening that is recommended on top of the guard: `sha_pinning_required` (every action pinned to a
commit SHA) and `allowed_actions=selected` (only listed actions may run).

The static checker is a safety net, **not** the barrier for fork pull requests. The barrier is the
repository settings below, which keep untrusted code away from the runner.

## Repository settings

Applied and verified by `ci/runner/secure-repo.ps1` (also called by `register.ps1`), through the REST
API with the `repo`-scoped token. Result of the first run, 2026-10-27 (each setting was applied if needed and read back with GET; the Result column says `verified` when the read-back matched):

| Setting | Endpoint | Before | After | Result |
|---|---|---|---|---|
| Fork PR workflows: require approval for all external contributors | `PUT /repos/dfsolutions/bamburino/actions/permissions/fork-pr-contributor-approval` (`approval_policy=all_external_contributors`) | `first_time_contributors` | `all_external_contributors` | verified |
| Default workflow permissions `read`, Actions cannot approve PRs | `PUT /repos/dfsolutions/bamburino/actions/permissions/workflow` | `read`, `can_approve_pull_request_reviews=false` | unchanged | verified |

Both endpoints worked with a `repo`-scoped PAT. If a future run reports `forbidden-by-scope`, set
by hand in Settings > Actions > General: *Fork pull request workflows from outside collaborators* >
**Require approval for all external contributors**, and *Workflow permissions* > **Read repository
contents and packages permissions** with *Allow GitHub Actions to create and approve pull requests*
unticked. Re-run `pwsh ci/runner/secure-repo.ps1` to verify; it exits non-zero if anything differs.

## Build parallelism

`driver/build.ps1` caps msbuild at `BAMBURINO_MSBUILD_JOBS` (default 8, integer 1..64) for the deps, app and test
steps. The `build-studio` job inherits that default; set the variable in the workflow `env:` only if a different cap
is needed on the runner.

## Tests

`pwsh ci/runner/tests/run.ps1` runs the unit tests (SHA-256 parsing, stop logic with fake processes and API, config arguments without
`--runasservice`, idempotent policy application against a fake API). They use no network.
