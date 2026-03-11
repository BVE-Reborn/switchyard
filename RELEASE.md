# Release Process

This document describes how to publish a new release of `switchyard`.

## Prerequisites

- Push access to the default branch
- A crates.io API token with publish rights for all workspace crates
- `gh` CLI installed and authenticated (for creating the GitHub release)

## Steps

### 1. Determine the new version

Pick the new version number following cargo semver conventions. For this document,
we'll use `X.Y.Z` as a placeholder.

### 2. Update CHANGELOG.md

**a) Add the new version to the Table of Contents:**

Find the line:
```
- [Unreleased](#unreleased)
```
Add a new entry directly below it:
```
- [vX.Y.Z](#vXYZ)
```
(The anchor is the version with dots removed, e.g. `v0.2.0` -> `#v020`)

**b) Add a version heading under Unreleased:**

Find:
```
## Unreleased
```
Add a blank line and a new version section below it, moving all existing unreleased
items under the new heading:
```
## Unreleased

## vX.Y.Z

Released YYYY-MM-DD

- (move all previously unreleased items here)
```

**c) Update the Diffs section at the bottom:**

Find the existing unreleased diff link:
```
- [Unreleased](https://github.com/BVE-Reborn/switchyard/compare/vPREVIOUS...HEAD)
```
Update it and add a new entry:
```
- [Unreleased](https://github.com/BVE-Reborn/switchyard/compare/vX.Y.Z...HEAD)
- [vX.Y.Z](https://github.com/BVE-Reborn/switchyard/compare/vPREVIOUS...vX.Y.Z)
```

### 3. Update Cargo.toml

Set the `version` field to the new version:
```toml
version = "X.Y.Z"
```

### 4. Update README.md

Update any version references (dependency snippets, compatibility tables, etc.)
to reflect the new version.

### 5. Commit and tag

```bash
jj commit -m "Release vX.Y.Z"
jj tag create vX.Y.Z
jj git push
```

### 6. Publish to crates.io

```bash
cargo publish
```

### 7. Create the GitHub release

Extract the release notes from `CHANGELOG.md` and create a release:

```bash
gh release create vX.Y.Z --title "vX.Y.Z" --notes "<paste release notes here>"
```

### 8. Post-release

Verify:
- [ ] The crate is visible at https://crates.io/crates/switchyard/X.Y.Z
- [ ] Docs are building at https://docs.rs/switchyard/X.Y.Z
- [ ] The GitHub release exists at https://github.com/BVE-Reborn/switchyard/releases/tag/vX.Y.Z
