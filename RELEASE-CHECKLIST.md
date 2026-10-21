# Release checklist

0.1.0 and 0.1.1 exist only on a private registry (test publishes of
2026-10-03 and 2026-10-04; `CHANGELOG.md` says why the version moved each time), so the
first crates.io release is 0.1.2.

Six crates are published from this workspace — `nlipsy-protocol`, the message model,
`nlipsy-core`, the runtime core over it, `nlipsy-http`, ECMA-431 §6 as values,
`nlipsy-http-client` and `nlipsy-server`, the two ends of that binding, and `nlipsy`,
the facade over them — and one member, `nlipsy-conformance`, is `publish = false` and
never leaves the repository. The version lives in the root manifest's
`[workspace.package]`, so a release is one operation over all six published crates: they
share a version and go out together. These steps assume a clean checkout of
`master` with everything to be released already merged.

## Publishing order, and why it is not a preference

`nlipsy-core` depends on `nlipsy-protocol`; `nlipsy-http` on both; `nlipsy-http-client`
and `nlipsy-server` on all three, and on neither each other; and `nlipsy` on all five, as
`{ path = "…", version = "0.1.2" }` — the path is what a workspace build uses, and the
version is what `cargo publish` rewrites it to, because a published crate cannot refer
to a directory on the machine that uploaded it. That fixes the order:
**`nlipsy-protocol`, then `nlipsy-core`, then `nlipsy-http`, then `nlipsy-http-client`,
then `nlipsy-server`, then `nlipsy`.** The two bindings are a pair rather than a chain —
neither depends on the other, which is the property the workspace exists to keep, and
`nlipsy-http` is what lets them agree on ECMA-431 §6 without one of them owning it — so
their relative order is free and is written down only so that there is one order to
follow. Two consequences, both mechanical:

- **Each crate is published only after the ones it depends on have landed.**
  Packaging a crate resolves its dependencies against the registry, so the dry run
  fails until they are there — with:

  ```
  error: failed to prepare local package for uploading

  Caused by:
    no matching package named `nlipsy-protocol` found
    location searched: crates.io index
    required by package `nlipsy v0.1.2 (…/crates/nlipsy)`
  ```

  (On a machine whose `~/.cargo/config.toml` replaces `crates-io` with a mirror, the
  index named is the mirror's.) So a dry run is
  not a pre-flight check for five of the six crates — it is a step that happens
  *after* the crates under them are on crates.io, and the first release is the one
  where that is most tempting to misread as a real failure.

  Spelled out, because the distinction is the difference between "wait" and "fix":

  | line | before anything is published |
  |---|---|
  | `cargo publish --dry-run -p nlipsy-protocol --registry crates-io` | **succeeds** — it depends on nothing in this workspace |
  | `cargo publish --dry-run -p nlipsy-core --registry crates-io` | **fails**, `no matching package named nlipsy-protocol` |
  | `cargo publish --dry-run -p nlipsy-http --registry crates-io` | **fails**, on `nlipsy-protocol` and `nlipsy-core` |
  | `cargo publish --dry-run -p nlipsy-http-client --registry crates-io` | **fails**, on `nlipsy-protocol`, `nlipsy-core` and `nlipsy-http` |
  | `cargo publish --dry-run -p nlipsy-server --registry crates-io` | **fails**, on `nlipsy-protocol`, `nlipsy-core` and `nlipsy-http` |
  | `cargo publish --dry-run -p nlipsy --registry crates-io` | **fails**, on all five |

  Every one of those five failures is expected, names the unpublished dependency, and
  clears itself the moment that dependency is on the index. A failure that names
  anything *else* — a path reaching outside the package, a missing README, a file the
  package should not carry — is a real one and must be fixed before step 8, because
  step 8 cannot be undone. That is what step 6 is for: `cargo package --list` answers
  those questions for every crate without needing the registry.

- **The `version` in `[workspace.dependencies]` must equal the version being
  published, for `nlipsy-protocol`, `nlipsy-core`, `nlipsy-http`, `nlipsy-http-client`
  and `nlipsy-server` alike.** Each
  is written out in full there (`version = "0.1.2"`) and does *not* follow
  `[workspace.package].version`, so bumping the release version and forgetting these
  lines publishes crates that depend on the previous release — which resolves, builds,
  and is wrong. Step 1 below changes all of them.

A `scripts/publish.sh` like jsonrpsee's is deliberately not here. That script exists
because jsonrpsee publishes eight crates in a dependency order nobody can be expected
to keep in their head; six crates in a fixed order are steps 7 to 9 of this file, and a
script would be one more thing to keep correct for no reduction in what has to be
understood. The count is six now and the judgement has not changed: revisit it when a
second *transport* arrives and the order stops being a straight line — that is when a
second `nlipsy-<standard>` crate joins `nlipsy-http` and the order stops being one.

## Before the first release only

- [ ] The GitHub repository `laktoosivaba/nlipsy` exists and `master` is pushed to
      it. The manifests' `repository`, `homepage` and the READMEs' licence links all
      point at it, and `cargo publish` verifies nothing about them — a broken link is
      discovered by the first reader, after it is permanent.
- [ ] `cargo login` has been done with a crates.io token that may publish new crates.
- [ ] All six names are free on crates.io — `nlipsy-protocol`, `nlipsy-core`,
      `nlipsy-http`, `nlipsy-http-client`, `nlipsy-server`, `nlipsy`. The bare `nlipsy`
      is the facade,
      not the message model, and it is published with real content from 0.1.2 rather
      than reserved with a placeholder — a name reserved is a promise to maintain it
      either way, so it goes out as the crate it is meant to be.

## Every release

1.  [ ] **Bump `version` in the root `Cargo.toml`** — in *six* places, as above:
        `[workspace.package].version`, which every member inherits with
        `version.workspace = true`, and the `version` field of
        `[workspace.dependencies].nlipsy-protocol`,
        `[workspace.dependencies].nlipsy-core`,
        `[workspace.dependencies].nlipsy-http`,
        `[workspace.dependencies].nlipsy-http-client` and
        `[workspace.dependencies].nlipsy-server`, which is what the published
        crates above them will ask the registry for.

        While the crates are 0.x, `0.x.0` is the breaking bump and `0.x.y` the
        compatible one. A raised MSRV is a breaking change and takes a minor bump of
        its own.

2.  [ ] **Finish the `CHANGELOG.md` section and date it.** Rename the heading from
        `unreleased, date TBD` to the release date in `YYYY-MM-DD`, open a fresh
        `## [Unreleased]` above it, and update the link definitions at the bottom.

        One changelog covers the workspace, as jsonrpsee's does: the crates share a
        version and ship together, so a reader looking up 0.2.0 should find every
        change in it in one place.

        Read the entries against the API as it actually ships, not as it was
        planned — every exported name mentioned should exist, in the crate it is
        attributed to.

3.  [ ] **`make check`** — `make check-features` first, then fmt, clippy over all
        targets and features, and the tests. Then the builds it does not cover, which
        CI runs and a release must not skip:

        ```
        cargo test --workspace                       # default features: no cbor
        cargo test -p nlipsy-core                    # the core with no features at all
        make check-wasm                              # wasm32, the members that promise it
        ```

        `make check-features` is every feature set `--all-features` cannot stand in
        for: the facade one feature at a time — thirteen lines, one per manifest a
        consumer might write, three of them pairs because `nlipsy::core`,
        `nlipsy::http` and `nlipsy::http_client` are each produced by more than one
        feature — then `nlipsy-http-client`'s own four (neither transport, hyper alone,
        both, and the hyper suite run), then `make check-mobile-graph`. It is in
        `make check` and it is named here because it is the check a release most needs
        and `--all-features` most reliably hides: `nlipsy` has no default features, so
        every module it has is behind one, and a feature that forgets part of what it
        enables compiles perfectly for everybody except the consumer who asked for only
        that feature — who cannot work around it without a patched manifest.

        `make check-mobile-graph` is the one gate in that target that is not a build.
        It reads `cargo tree -p nlipsy-http-client --no-default-features --features
        hyper -e normal,build` for `aarch64-linux-android` and `aarch64-apple-ios` and
        fails if `jni`, `rustls-platform-verifier`, `aws-lc`, `cmake`,
        `security-framework`, `openssl` or `native-tls` is in either tree, printing the
        three TLS crates that should be (`ring`, `webpki-roots`, `cc`). A release must
        not skip it: that graph is the whole reason `nlipsy-http-client` has a second
        transport, a published crate is where it stops being this workspace's problem,
        and one transitive default re-enabled three crates down would restore the whole
        of it while compiling perfectly everywhere this checklist is run. `cargo tree
        --target` needs neither target installed nor an NDK nor Xcode.

        The facade's doctests are the other thing a release must see run. They live on
        the modules rather than in the README — the README is the crate documentation
        and is compiled by the no-feature run, where none of the modules exists — so
        they run under `cargo test --workspace --all-features` and nowhere else. Three
        of them, one per role: `protocol`, `http_client`, `server`. The `http_client`
        one picks its transport with a `#[cfg]`, because the module is produced by two
        features and `HttpTransport` exists under only one of them.

        `make check-wasm` excludes the two native HTTP bindings and the facade, and
        explains why: the wasm promise covers the crates a JavaScript package compiles,
        a native HTTP binding — the client over reqwest or over hyper, or the axum
        server — is not one of them, and `--all-features` on the facade means `full`,
        which means both. The
        facade is checked on a second line, with the features whose crates do compile
        there. The `-p nlipsy-core` line is there because `--workspace` no longer
        reaches the core's feature-less build: the HTTP client's dev-dependency asks for
        `client` and `testing`, and unification turns them on for the whole run.

        `cargo test --workspace` matters twice over now: `nlipsy` has *no* default
        features, so that run is the one that proves the facade still compiles as the
        empty crate a bare `nlipsy = "0.1"` gets.

4.  [ ] **Documentation builds clean**, the way docs.rs will build it:

        ```
        RUSTDOCFLAGS="-D warnings --cfg docsrs" cargo +nightly doc --workspace --no-deps --all-features
        ```

        Nightly because `doc_cfg` is nightly-only, and `--cfg docsrs` because that is
        what docs.rs sets. A rustdoc warning found here is free; found after
        publication it is a new version.

5.  [ ] **`cargo deny check all`** passes on the lockfile being released.

6.  [ ] **Review the package contents of every published crate:**

        ```
        cargo package -p nlipsy-protocol --list
        cargo package -p nlipsy-core --list
        cargo package -p nlipsy-http --list
        cargo package -p nlipsy-http-client --list
        cargo package -p nlipsy-server --list
        cargo package -p nlipsy --list
        ```

        Read the whole list. Each must contain its own `README.md` (it is the crate
        documentation via `include_str!`, so the package does not build without it)
        and both licence files from its own package root — they are copies in each
        crate directory, because Cargo only picks up licence files automatically when
        they sit at the package root.

        `nlipsy-http-client` also ships `tests/fixtures/recorded-wire.jsonl`, and that
        is deliberate: the recorded wire is evidence from outside this workspace's own
        encoder, the test beside it is what reads it, and a package whose test data was
        left behind is a test that cannot be run from the published sources.

        None of them may contain **anything from `ecma-spec-md/`**. Those are Ecma
        International's documents, they are excluded through `.git/info/exclude`
        rather than `.gitignore`, and `cargo package` falls back to walking the
        directory when git does not answer — so this is a real possibility and not a
        formality. A published `.crate` cannot be unpublished.

7.  [ ] **`cargo publish --dry-run -p nlipsy-protocol --registry crates-io`.** This
        packages and builds the crate from the packaged sources, which is where a path
        that reaches outside the package surfaces. It is the one dry run that can
        succeed before anything is published; see the table at the top of this file.

        If `~/.cargo/config.toml` replaces `crates-io` with a mirror, every publish
        command needs
        `--registry crates-io` appended, or cargo refuses with *"crates-io is
        replaced with non-remote-registry source"*. The replacement is for fetching;
        publishing has to reach the real registry.

8.  [ ] **`cargo publish -p nlipsy-protocol --registry crates-io`.** Irreversible: a
        version number can be yanked but never reused.

9.  [ ] **Then each crate once the index holds the ones under it: the core, then the
        HTTP client, then the server, then the facade.**

        ```
        cargo publish --dry-run -p nlipsy-core --registry crates-io
        cargo publish -p nlipsy-core --registry crates-io

        cargo publish --dry-run -p nlipsy-http --registry crates-io
        cargo publish -p nlipsy-http --registry crates-io

        cargo publish --dry-run -p nlipsy-http-client --registry crates-io
        cargo publish -p nlipsy-http-client --registry crates-io

        cargo publish --dry-run -p nlipsy-server --registry crates-io
        cargo publish -p nlipsy-server --registry crates-io

        cargo publish --dry-run -p nlipsy --registry crates-io
        cargo publish -p nlipsy --registry crates-io
        ```

        The dry run is here rather than beside step 7 for the reason at the top of
        this file — before step 8 it can only fail on the dependency it cannot
        resolve. If it fails on that *after* step 8, the index has not caught up yet;
        wait rather than reaching for `--allow-dirty` or a version edit.

10. [ ] **Commit and tag.** The tag is annotated, and signed if a signing key is
        configured:

        ```
        git tag -s v0.1.2 -m "nlipsy 0.1.2"   # or -a, with no signing key
        git push origin master --follow-tags
        ```

        One tag covers the release, because one version does. `v*` tags are a CI
        trigger, so the tag build is also the confirmation that the released tree is
        the one that passes.

11. [ ] **Draft the GitHub release** against that tag, with the changelog section for
        this version as the body.

12. [ ] **After the first publish only:** remove `continue-on-error: true` and its
        `TODO` from the `semver` job in `.github/workflows/ci.yml`. Until a version
        exists on crates.io, `cargo-semver-checks` has no baseline to compare a pull
        request against and can only fail for want of one; from 0.1.2 onward its
        result is meaningful and should block. It checks every publishable member, so
        one removal covers all six.

13. [ ] **Check <https://docs.rs/nlipsy-protocol>, <https://docs.rs/nlipsy-core>,
        <https://docs.rs/nlipsy-http>, <https://docs.rs/nlipsy-http-client>,
        <https://docs.rs/nlipsy-server> and
        <https://docs.rs/nlipsy>** once the builds land. docs.rs builds with `--cfg docsrs` and all features — the
        settings recorded in each crate's `[package.metadata.docs.rs]` — so step 4
        should have caught anything, but the rendered page is the thing users see. On
        the facade, check that the modules carry their "Available on crate feature"
        banners: with no features there is nothing on that page at all, and a missing
        `all-features = true` would look exactly like a broken build.
