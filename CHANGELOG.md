# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Each change links to its implementing commits. Commit lists cover the immutable
release tag ranges; later acceptance receipts retain their actual follow-up
commits and original release attribution. Uncommitted work is marked explicitly.

## [Unreleased]

## [0.6.1] - 2026-10-18

### Added

- A compatibility foundation for 0.6.1: capability-aware resource readers preserve opaque future fields, pause only unsupported builds and continue resolving published roots. Compatible additive CRDs remain installed during binary rollback, subject to reader contracts, structural checks and ownership validation. Schema revision 6 and existing wire formats remain unchanged; published 0.6.0 remains a tested rollback target before newer features are used. Commits: [b0c8464](https://github.com/inferal-oss/kimage/commit/b0c8464f5d3432414f4ef9369404980f5f4c3c1a).

### Changed

- Recorded [eight native CLI/Helm and four Argo CD cases](https://github.com/inferal-oss/kimage/blob/main/docs/native-0.6.1-results.json), including actual published 0.6.0 round trips and unchanged-baseline rollback after an independent future writer populates schema 7, native journals and provenance. [Real mixed-cluster acceptance](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-0.6.1-results.json) repeats all three managers with interruptions, native placement and retained-data reinstall, preserving the permanent installation's 247 release contracts, nine credentials, storage, policy and 62 complete OCI graphs. Commits: [8170281](https://github.com/inferal-oss/kimage/commit/8170281ac396d65281b55342d11b6e0777084e54).

- Native publication verifies bounded future-schema catalogs and their complete native graphs, allowing the independent forward-reader fixture to publish candidate images. Commits: [8244ba6](https://github.com/inferal-oss/kimage/commit/8244ba600f70a40ba2fed2e1e92b4fe7ddd153ad).

- Release validation accepts pending version work under Unreleased; final release packaging still requires dated version notes. Commits: [b0c8464](https://github.com/inferal-oss/kimage/commit/b0c8464f5d3432414f4ef9369404980f5f4c3c1a).

### Fixed

- Lifecycle lock acquisition retries resource-version conflicts using a fresh ownership and expiry check, so concurrent node reconciliation cannot spuriously abort uninstall or another transition. Active and foreign holders remain protected. Commits: [ef5829c](https://github.com/inferal-oss/kimage/commit/ef5829cbb3ead21994ef4d44cabbb1d080fe81a2).

- CLI interruption acceptance holds the response after a durable Applying journal write, making fast cached transitions deterministic without production test hooks. Resume restores the original connection. Commits: [416a3fe](https://github.com/inferal-oss/kimage/commit/416a3fe357d5c20b6f820575b0da0036d0d6e847).

- Lifecycle preflight verifies the shared schema contract against the requested target reader, refusing a too-old prepared target before runtime or journal changes. Commits: [cdad262](https://github.com/inferal-oss/kimage/commit/cdad2622f161fff30dd0f49af1c7ce33661d2970).

- Future-version acceptance uses the complete Helm chart, preserving fixture credentials and runtime resources through interrupted upgrades; chart packaging rejects incomplete runtime charts. UID-guarded consumer cleanup removes each fixture once before verifying uninstall retention. Commits: [b174932](https://github.com/inferal-oss/kimage/commit/b174932000b9ae6e30519c8a3df181c2d2bf6a99), [da52b0f](https://github.com/inferal-oss/kimage/commit/da52b0ffef02d8a934fa531478c72b6477b33985).

- New build-journal and execution-receipt fields pause the affected build and retain their original data, even when historical typed decoders would ignore extra fields. Commits: [f6ee5a3](https://github.com/inferal-oss/kimage/commit/f6ee5a33514729b549d30b0ffc93a4970203e98e).

- Unrelated opaque status metadata no longer causes repeated writes when a prepared older reader has no status changes to make. Known status updates still preserve the newer fields. Commits: [9d42078](https://github.com/inferal-oss/kimage/commit/9d42078c93fa2fbbacaab1c560f570ccef7ac49c).

- Fresh Images with unsupported requirements receive a valid paused status before any build starts, while existing readiness and opaque state remain untouched. Commits: [8b177bf](https://github.com/inferal-oss/kimage/commit/8b177bfa0cb1673bd1ee4382e0ae00e651da68de).

**Commits:** [2ca96a9](https://github.com/inferal-oss/kimage/commit/2ca96a9c318a476b79fa5cf7f4b6371477087df1), [52cbc77](https://github.com/inferal-oss/kimage/commit/52cbc778cdba0bfcbaca347342a1feb95e6bc652), [b0c8464](https://github.com/inferal-oss/kimage/commit/b0c8464f5d3432414f4ef9369404980f5f4c3c1a), [1713d8c](https://github.com/inferal-oss/kimage/commit/1713d8cfd15aa9dec34dd6954ff0adb3338fa7f0), [8b177bf](https://github.com/inferal-oss/kimage/commit/8b177bfa0cb1673bd1ee4382e0ae00e651da68de), [40e6001](https://github.com/inferal-oss/kimage/commit/40e60012c5b6de946fb8cff77068212fd3631107), [8244ba6](https://github.com/inferal-oss/kimage/commit/8244ba600f70a40ba2fed2e1e92b4fe7ddd153ad), [9d42078](https://github.com/inferal-oss/kimage/commit/9d42078c93fa2fbbacaab1c560f570ccef7ac49c), [f6ee5a3](https://github.com/inferal-oss/kimage/commit/f6ee5a33514729b549d30b0ffc93a4970203e98e), [b174932](https://github.com/inferal-oss/kimage/commit/b174932000b9ae6e30519c8a3df181c2d2bf6a99), [da52b0f](https://github.com/inferal-oss/kimage/commit/da52b0ffef02d8a934fa531478c72b6477b33985), [cdad262](https://github.com/inferal-oss/kimage/commit/cdad2622f161fff30dd0f49af1c7ce33661d2970), [5837f0f](https://github.com/inferal-oss/kimage/commit/5837f0f5a156e8d282b36de91b7111b9b14b29d5), [749c2ea](https://github.com/inferal-oss/kimage/commit/749c2ea14ec4d30d29b8455c94af70a1ffe785ec), [416a3fe](https://github.com/inferal-oss/kimage/commit/416a3fe357d5c20b6f820575b0da0036d0d6e847), [ef5829c](https://github.com/inferal-oss/kimage/commit/ef5829cbb3ead21994ef4d44cabbb1d080fe81a2), [8170281](https://github.com/inferal-oss/kimage/commit/8170281ac396d65281b55342d11b6e0777084e54).

## [0.6.0] - 2026-10-17

### Added

- Frozen OCI base and toolchain image inputs for multi-stage Containerfiles. Named inputs select exact, already published same-namespace Image versions; native builds use their verified retained bytes without registry access from BuildKit. Commits: [83abe62](https://github.com/inferal-oss/kimage/commit/83abe62796e057da7c32665d40141d44c685acbb).

- A dedicated, rendered website changelog linked from the top navigation, regenerated from these notes on every GitHub Pages build. Commits: [9c1636e](https://github.com/inferal-oss/kimage/commit/9c1636e6863bab0a018e1745bab407ecab39dcb6).

### Changed

- Recorded [post-publication 0.6.0 verification](https://github.com/inferal-oss/kimage/blob/main/docs/published-0.6.0-results.json): exact-tag assets and both crates, public native GHCR indexes, Cargo installation and automatic catalog fetching. The permanent mixed cluster runs the official public Helm chart through Argo CD with built-in pins, healthy status/doctor, retained storage, 203 prior release contracts, nine credentials and policy, and complete readback of 18 prior and 35 new graphs. Post-publication follow-up: [2ca96a9](https://github.com/inferal-oss/kimage/commit/2ca96a9c318a476b79fa5cf7f4b6371477087df1).

- Recorded [eight native CLI/Helm and four Argo CD cases](https://github.com/inferal-oss/kimage/blob/main/docs/native-0.6.0-results.json) and [real mixed-cluster frozen-input acceptance](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-0.6.0-results.json). Native placement, cache controls, refusal cases and later platform demand retain the original dependency roots. The permanent upgrade preserves 203 prior release contracts, nine credentials, storage and policy, with complete readback of 18 prior and 35 new graphs. Commits: [a3cb744](https://github.com/inferal-oss/kimage/commit/a3cb744812e54fcd29dfb37fc7d8b12905ca003e).
- Optional encrypted Helm diagnostics capture disposable CI runtime state and bounded component logs after a failed install, while keeping plaintext out of artifacts and output. Commits: [045cbd7](https://github.com/inferal-oss/kimage/commit/045cbd7a3b47bd3b6bf067178169abbb13964b40).
- Website footers now credit Inferal as sponsor, with its chevron logo and a link to inferal.com. Commits: [7615d86](https://github.com/inferal-oss/kimage/commit/7615d862a0b0c1aa249d777a9bd32c1b878b0bf7).

- Historical changelog entries now link to implementing commits, with complete release commit lists and support for associated pull request links. Repository instructions require this traceability for future changes. Commits: [9c1636e](https://github.com/inferal-oss/kimage/commit/9c1636e6863bab0a018e1745bab407ecab39dcb6).

- Helm installation and Argo provisioning catalogs declare schema revision 6 consistently. Commits: [69bea45](https://github.com/inferal-oss/kimage/commit/69bea45323e5a4bce18bfb1ff4c3bc6120f958c6).

- Native acceptance covers published 0.5.2 upgrades, interruption recovery, schema downgrade refusal, frozen OCI input execution and retained-data reinstall. Fresh-install checks retire only owned fixture caches to require cold pulls. Optional diagnostics encrypt private logs and publish bounded status codes. Commits: [83abe62](https://github.com/inferal-oss/kimage/commit/83abe62796e057da7c32665d40141d44c685acbb), [ab89aa8](https://github.com/inferal-oss/kimage/commit/ab89aa8d1d6c5a7b7118bff4a333c6047d7e38d6), [1e7d72b](https://github.com/inferal-oss/kimage/commit/1e7d72bd39c371e6d2aa6cbacc7806b4515a3637), [807263b](https://github.com/inferal-oss/kimage/commit/807263b3de0d0ddaf4484fd5c4a813045ce2f64a), [23145c6](https://github.com/inferal-oss/kimage/commit/23145c6abe03f5b4a75eac5d070f985bb13dcef4).

- The guarded 0.5.2 to 0.6.0 upgrade adds shared CRD revision 6 without changing registry storage or serving protocol. Rollback to 0.5.2 is refused because it cannot read the new fields; earlier same-schema edges remain supported. Commits: [83abe62](https://github.com/inferal-oss/kimage/commit/83abe62796e057da7c32665d40141d44c685acbb).

### Fixed

- Helm lifecycle acceptance reinstalls the target from its verified native fixture mirror with the exact target digest pins, before public release images exist. Commits: [7b6346a](https://github.com/inferal-oss/kimage/commit/7b6346a0a2c1cc36c57d040add45831939c2a1a0).
- Argo acceptance uses an undeclared future target for its refusal check, so releasing 0.6.0 cannot turn the negative case into a valid same-version request. Commits: [44ed37f](https://github.com/inferal-oss/kimage/commit/44ed37f9e876cc2f4092d6bbd2bcdf81c3f0b2a8).
- Namespace-scoped operators can derive session read keys by getting only their installation's named reader credential Secret, without broader system Secret access. Commits: [0bf384c](https://github.com/inferal-oss/kimage/commit/0bf384cb675ebe05dfd35e7a8121be7508eaa6d4).
- Native lifecycle acceptance uses Helm to remove Helm-owned installations before retained-data reinstall, respecting the CLI manager guard. Commits: [67bc553](https://github.com/inferal-oss/kimage/commit/67bc55386317bdbdaec28484b4a8e81664927077).
- After reinstalling, published consumers authenticate their frozen dependency bindings from retained verified release provenance when regenerated service credentials no longer match the original snapshot signature. Names are never resolved again. Commits: [67bc553](https://github.com/inferal-oss/kimage/commit/67bc55386317bdbdaec28484b4a8e81664927077).
- Native Linux builds stage frozen OCI inputs without cross-device link errors, with temporary verification files and staged layouts on the same mount. Commits: [4f99344](https://github.com/inferal-oss/kimage/commit/4f99344e77fe39b15ed087058c208b2ea5504f5a).
- Application build sessions receive namespace-only registry read credentials. The installation-wide reader password stays in the system namespace; other namespaces and all writes are refused with a session key. Commits: [0188dec](https://github.com/inferal-oss/kimage/commit/0188dec7643bc74d573e31a525bdbdcc78ab4e72).
- Input-capture failures retain error context in the bounded private build log. Public status and operator output keep stable reason codes. Commits: [5825e5c](https://github.com/inferal-oss/kimage/commit/5825e5c1fd63d56fa2671d3e2e38472079c90498).
- Upgrade checks report the target API schema revision, including revision 6. Commits: [8594529](https://github.com/inferal-oss/kimage/commit/85945293122bd471a2491d6d0c69f100ee733668).

**Commits:** [fc2664a](https://github.com/inferal-oss/kimage/commit/fc2664af53be0822adb7882e1fcea6672df27a49), [7615d86](https://github.com/inferal-oss/kimage/commit/7615d862a0b0c1aa249d777a9bd32c1b878b0bf7), [9c1636e](https://github.com/inferal-oss/kimage/commit/9c1636e6863bab0a018e1745bab407ecab39dcb6), [a223345](https://github.com/inferal-oss/kimage/commit/a223345ec8200feb4ebc43b1a60d2830c334a0c9), [83abe62](https://github.com/inferal-oss/kimage/commit/83abe62796e057da7c32665d40141d44c685acbb), [2531858](https://github.com/inferal-oss/kimage/commit/2531858e00f8a800cc37df6d29b2e4d9d6f377fd), [69bea45](https://github.com/inferal-oss/kimage/commit/69bea45323e5a4bce18bfb1ff4c3bc6120f958c6), [ab89aa8](https://github.com/inferal-oss/kimage/commit/ab89aa8d1d6c5a7b7118bff4a333c6047d7e38d6), [8594529](https://github.com/inferal-oss/kimage/commit/85945293122bd471a2491d6d0c69f100ee733668), [1e7d72b](https://github.com/inferal-oss/kimage/commit/1e7d72bd39c371e6d2aa6cbacc7806b4515a3637), [807263b](https://github.com/inferal-oss/kimage/commit/807263b3de0d0ddaf4484fd5c4a813045ce2f64a), [5825e5c](https://github.com/inferal-oss/kimage/commit/5825e5c1fd63d56fa2671d3e2e38472079c90498), [4f99344](https://github.com/inferal-oss/kimage/commit/4f99344e77fe39b15ed087058c208b2ea5504f5a), [0188dec](https://github.com/inferal-oss/kimage/commit/0188dec7643bc74d573e31a525bdbdcc78ab4e72), [23145c6](https://github.com/inferal-oss/kimage/commit/23145c6abe03f5b4a75eac5d070f985bb13dcef4), [0d879d8](https://github.com/inferal-oss/kimage/commit/0d879d846d739235f16f1cf0bb3049b038ae153c), [67bc553](https://github.com/inferal-oss/kimage/commit/67bc55386317bdbdaec28484b4a8e81664927077), [0bf384c](https://github.com/inferal-oss/kimage/commit/0bf384cb675ebe05dfd35e7a8121be7508eaa6d4), [0f80480](https://github.com/inferal-oss/kimage/commit/0f80480823f29fd00215470c576dbf4f9f83be3b), [44ed37f](https://github.com/inferal-oss/kimage/commit/44ed37f9e876cc2f4092d6bbd2bcdf81c3f0b2a8), [90544cf](https://github.com/inferal-oss/kimage/commit/90544cfcbe1f9d53e1d3d60fc9171579100700dc), [045cbd7](https://github.com/inferal-oss/kimage/commit/045cbd7a3b47bd3b6bf067178169abbb13964b40), [7b6346a](https://github.com/inferal-oss/kimage/commit/7b6346a0a2c1cc36c57d040add45831939c2a1a0), [393cc74](https://github.com/inferal-oss/kimage/commit/393cc7403c381e73be2749743919d217ba0a93c4), [a3cb744](https://github.com/inferal-oss/kimage/commit/a3cb744812e54fcd29dfb37fc7d8b12905ca003e), [b6e72be](https://github.com/inferal-oss/kimage/commit/b6e72bee5e87c72b3f9cce45539840b91598e445).


## [0.5.2] - 2026-10-17

### Added

- `kimage installations` lists installation identities, namespaces, managers, releases and scope, with namespace filtering and JSON output. Commits: [648484a](https://github.com/inferal-oss/kimage/commit/648484a503a0e90a685ba9e2a5e4dafd1a82e248).
- Status and doctor automatically discover installations when the default is absent and inspect every match, continuing after individual failures. Upgrade, rollback and uninstall discover a single target or list explicit choices when several match. `--no-discovery` disables fallback; explicit selectors constrain it. Commits: [648484a](https://github.com/inferal-oss/kimage/commit/648484a503a0e90a685ba9e2a5e4dafd1a82e248).
- CLI upgrade and rollback direct Argo-managed installations to their Application in Git; read-only compatibility checks remain available. Commits: [d0d4a99](https://github.com/inferal-oss/kimage/commit/d0d4a99ae60bedc3534a68a1401d0ad2fd15bcaa).
- Argo CD runtime uninstall requires terminal confirmation or `--allow-argocd`, while retaining active-provisioner, lifecycle and data protections. Commits: [648484a](https://github.com/inferal-oss/kimage/commit/648484a503a0e90a685ba9e2a5e4dafd1a82e248).

### Changed

- Recorded [eight native CLI/Helm cases and four Argo CD cases](https://github.com/inferal-oss/kimage/blob/main/docs/native-0.5.2-results.json), including published 0.5.1 upgrades, interruptions, rollback and retained-image recovery. Commits: [d0d4a99](https://github.com/inferal-oss/kimage/commit/d0d4a99ae60bedc3534a68a1401d0ad2fd15bcaa).
- Verified [published artifacts and a permanent mixed-cluster Argo upgrade](https://github.com/inferal-oss/kimage/blob/main/docs/published-0.5.2-results.json): Cargo installation, public Helm pins, native placement before/after upgrading, retained identities and complete OCI graph readback. Default discovery finds the healthy cluster-wide installation. Commits: [fc2664a](https://github.com/inferal-oss/kimage/commit/fc2664af53be0822adb7882e1fcea6672df27a49).

### Fixed

- Cluster-wide upgrade checks exclude historical releases created before the retained registry claim, preventing unrelated retired installations from blocking a new registry's transition. Releases with missing timestamps remain checked. Commits: [650df76](https://github.com/inferal-oss/kimage/commit/650df76dd225417493a28ff2a498840636d474ea).
- Missing installation errors explain which identity was requested and how to find or select an installation. Permission and API failures stay distinct from an absent installation. Commits: [648484a](https://github.com/inferal-oss/kimage/commit/648484a503a0e90a685ba9e2a5e4dafd1a82e248).

**Commits:** [b3c8388](https://github.com/inferal-oss/kimage/commit/b3c83882a005da8168fb935d1fda66473c5845bb), [648484a](https://github.com/inferal-oss/kimage/commit/648484a503a0e90a685ba9e2a5e4dafd1a82e248), [650df76](https://github.com/inferal-oss/kimage/commit/650df76dd225417493a28ff2a498840636d474ea), [d0d4a99](https://github.com/inferal-oss/kimage/commit/d0d4a99ae60bedc3534a68a1401d0ad2fd15bcaa).

## [0.5.1] - 2026-10-16

### Added

- Argo CD chart mode with deterministic offline rendering, an in-cluster provisioner, automatic native node discovery and generated or externally supplied credentials. Applications expose readiness through the provisioner Deployment and support both installation scopes. Commits: [0149688](https://github.com/inferal-oss/kimage/commit/01496888f35c8b438a00b7f686a92505b7cb76af).
- Argo CD ownership, guarded runtime transitions and explicit removal that preserves registry data, Images, releases and shared CRDs. Existing CLI/Helm installations are not adopted automatically; Argo CD ownership requires 0.5.1 or newer. Commits: [0149688](https://github.com/inferal-oss/kimage/commit/01496888f35c8b438a00b7f686a92505b7cb76af), [0435b10](https://github.com/inferal-oss/kimage/commit/0435b10bda1f3dde5ef90aeef536922c0c041ef6).
- Argo CD chart-version upgrades and compatible downgrades from 0.5.1 onward. A returning older chart automatically delegates to the installed newer operator by digest, preserving the lifecycle lock and resumable journal, without manual catalog selection. Commits: [972d6c8](https://github.com/inferal-oss/kimage/commit/972d6c89db48aff9761f645768ef47d2e52e1f57).
- Copyable Argo CD Applications and installation, configuration, health and retention guidance in the README, handbook and agent essentials. Commits: [0149688](https://github.com/inferal-oss/kimage/commit/01496888f35c8b438a00b7f686a92505b7cb76af), [ed016cb](https://github.com/inferal-oss/kimage/commit/ed016cb624ea61b053276db16a5b677f752531ee).

### Changed

- Verified the published Cargo CLI and public Helm repository through real Argo CD with the chart's built-in pins, native amd64/arm64 builds and retained data after removal/reinstall. The [published-artifact receipt](https://github.com/inferal-oss/kimage/blob/main/docs/published-0.5.1-results.json) also checks automatic catalog fetching, public asset checksums and anonymous component access. Commits: [b3c8388](https://github.com/inferal-oss/kimage/commit/b3c83882a005da8168fb935d1fda66473c5845bb).
- Recorded [native acceptance](https://github.com/inferal-oss/kimage/blob/main/docs/native-0.5.1-results.json) and [real mixed-cluster acceptance](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-0.5.1-results.json), including both Argo CD scopes, interrupted chart-version upgrades/downgrades, native placement and retained-data readback. Earlier attempts remain recorded alongside the complete repeat. Commits: [111650a](https://github.com/inferal-oss/kimage/commit/111650abc2ea1433668c615c724871778fdbee17), [ed016cb](https://github.com/inferal-oss/kimage/commit/ed016cb624ea61b053276db16a5b677f752531ee), [4bba1fd](https://github.com/inferal-oss/kimage/commit/4bba1fd746ac27f305acb9df743643f167fe84ad).
- CLI and Helm transition acceptance now targets published 0.5.0 to 0.5.1 and back. Published 0.5.0 resource templates are frozen for rollback; historical acceptance evidence remains unchanged. Commits: [bea2a36](https://github.com/inferal-oss/kimage/commit/bea2a367938c9243573250cecad27464aef68c1d), [ed016cb](https://github.com/inferal-oss/kimage/commit/ed016cb624ea61b053276db16a5b677f752531ee).

### Fixed

- Helm upgrades from 0.5.0 tolerate reused values without an Argo CD configuration section. Commits: [972d6c8](https://github.com/inferal-oss/kimage/commit/972d6c89db48aff9761f645768ef47d2e52e1f57).
- Release instructions use the supported adjacent 0.5.0 to 0.5.1 upgrade and reverse transition. Argo removal guidance and acceptance wait for provisioner Pods to finish before runtime cleanup. Commits: [ed016cb](https://github.com/inferal-oss/kimage/commit/ed016cb624ea61b053276db16a5b677f752531ee).
- Argo CD transitions verify retained image bytes through the authenticated in-cluster registry service, preserving TLS, root digest and size checks without API port-forward privileges. Commits: [c83455e](https://github.com/inferal-oss/kimage/commit/c83455eb304d2145cad088ea9ca6874088dcab9e).
- Argo CD runtime uninstall waits for downgrade helper Jobs and Pods to disappear, preventing them from racing explicit removal. Commits: [4bba1fd](https://github.com/inferal-oss/kimage/commit/4bba1fd746ac27f305acb9df743643f167fe84ad).
- Installation and GitOps policy updates recheck installation identity and pending operations after acquiring the lifecycle lock, refusing stale preflight state before applying resources. Commits: [0435b10](https://github.com/inferal-oss/kimage/commit/0435b10bda1f3dde5ef90aeef536922c0c041ef6).

**Commits:** [25213c3](https://github.com/inferal-oss/kimage/commit/25213c39540c63cb3a9992a977c73a288f354975), [0149688](https://github.com/inferal-oss/kimage/commit/01496888f35c8b438a00b7f686a92505b7cb76af), [0435b10](https://github.com/inferal-oss/kimage/commit/0435b10bda1f3dde5ef90aeef536922c0c041ef6), [c84c6ad](https://github.com/inferal-oss/kimage/commit/c84c6ad120142b14b9939f0d70932412c87801a5), [bea2a36](https://github.com/inferal-oss/kimage/commit/bea2a367938c9243573250cecad27464aef68c1d), [972d6c8](https://github.com/inferal-oss/kimage/commit/972d6c89db48aff9761f645768ef47d2e52e1f57), [3c8218c](https://github.com/inferal-oss/kimage/commit/3c8218cf6d759b51112ac974e561bd07f7719da7), [3017ae9](https://github.com/inferal-oss/kimage/commit/3017ae91d21783a77e9b3760b2cf9ab50432c9b1), [74f07aa](https://github.com/inferal-oss/kimage/commit/74f07aa947ef8cf58ea07d34dafe509c4ab21c5d), [c83455e](https://github.com/inferal-oss/kimage/commit/c83455eb304d2145cad088ea9ca6874088dcab9e), [8d67bef](https://github.com/inferal-oss/kimage/commit/8d67befbf9b347b38004dc957d547ddf98e71206), [111650a](https://github.com/inferal-oss/kimage/commit/111650abc2ea1433668c615c724871778fdbee17), [4bba1fd](https://github.com/inferal-oss/kimage/commit/4bba1fd746ac27f305acb9df743643f167fe84ad), [ed016cb](https://github.com/inferal-oss/kimage/commit/ed016cb624ea61b053276db16a5b677f752531ee).

## [0.5.0] - 2026-10-16

### Added

- Explicit installation upgrade and compatible rollback commands, with read-only checks, an exclusive Lease, resumable phases and retained storage/credential identity verification. The initial supported transition is 0.4.1 to 0.5.0 and back, without storage or API migration. Commits: [5f480cf](https://github.com/inferal-oss/kimage/commit/5f480cf78a0fe8af54d898bbb1482757d288d8f5).
- Automatic node membership and native worker reconciliation, with UID-bound serving approvals, administrator node selection and worker pins, and fencing before replacement. Commits: [5f480cf](https://github.com/inferal-oss/kimage/commit/5f480cf78a0fe8af54d898bbb1482757d288d8f5), [28789ba](https://github.com/inferal-oss/kimage/commit/28789ba22bafaa0ff01335ca4db9e9c6b0701a19).
- Read-only `kimage status` and `kimage doctor`, including JSON output, pending operations, component readiness, native platforms, retained storage, certificate validity and resource drift. Commits: [5f480cf](https://github.com/inferal-oss/kimage/commit/5f480cf78a0fe8af54d898bbb1482757d288d8f5).
- Private build diagnostics through `kimage logs IMAGE --follow` and `kimage builds IMAGE`, with platform and attempt selection. Completed attempt logs survive build Pod cleanup; each platform retains the latest five attempts with a bounded 64 KiB output tail per attempt. Guides explain access permissions, retry policy, output clipping and progress limits. Commits: [5f480cf](https://github.com/inferal-oss/kimage/commit/5f480cf78a0fe8af54d898bbb1482757d288d8f5), [81140cb](https://github.com/inferal-oss/kimage/commit/81140cb0c094f65e11c3354d3c5046e085b88de9).

### Changed

- Recorded [eight native lifecycle cases](docs/native-upgrade-results.json) and [real mixed-cluster CLI/Helm round trips](docs/mixed-upgrade-results.json) through actual published 0.4.1, including interruption/resume, native pulls/builds, node eligibility/worker changes, private failure logs and retained-image recovery after uninstall/reinstall. Commits: [9d868b4](https://github.com/inferal-oss/kimage/commit/9d868b496c53e14a81027265c8706f5cb8a5b8f1).

- After rollback to the published 0.4.1 controller, the documented Helm removal procedure stops its operator and retires its recreated workers before chart removal. Acceptance guards worker identity/specification, recognizes Kubernetes' omitted false defaults, and refuses foreign Kubernetes or Helm owners. Fresh 0.5.0 Helm removal is tested directly. Commits: [f0d1475](https://github.com/inferal-oss/kimage/commit/f0d14755f81ccdfc819d994a5e52aceb47e97ebe), [2f0abde](https://github.com/inferal-oss/kimage/commit/2f0abde3ce88dd848600b216294b0079f8593103), [b6bab31](https://github.com/inferal-oss/kimage/commit/b6bab319f041e640e33264cf7299f19f703bc6db).

### Fixed

- Guarded Helm transitions coordinate proxy and worker replacement with the predecessor operator, recover interrupted history without immutable Job conflicts, and tolerate transient restart backoff with Helm 4's legacy readiness wait. Ordinary three-way updates preserve Helm ownership and revision history. Commits: [8de46e5](https://github.com/inferal-oss/kimage/commit/8de46e5c89e5699de2edae69fa8216f56013f00d), [c44d092](https://github.com/inferal-oss/kimage/commit/c44d0926c0e3ebd21fcc39ed05a4a9ee861f9891), [c9ecc2d](https://github.com/inferal-oss/kimage/commit/c9ecc2d068f5a0bf85bb18ac1bde16bc858e3d10).
- CLI rollback resume stops partially restarted predecessor components again. Concurrent worker creation is accepted only for the exact owned, non-deleting specification. Uninstall rechecks pending operations and installation identity under its Lease and stops on Lease loss. Commits: [38e9bfb](https://github.com/inferal-oss/kimage/commit/38e9bfbd673498fd19bf845223daa45162eedea8), [b02175f](https://github.com/inferal-oss/kimage/commit/b02175fc904dfb756e9df5917df85f27ef8f47ca), [4cfddd3](https://github.com/inferal-oss/kimage/commit/4cfddd3ef8800bf578ab40298cb1a0a71bc33480).
- Node reconciliation repairs missing ownership on unchanged resources without restarting them or taking over another owner. Worker recovery and operation journals retain installation ownership for chart adoption and cleanup. Required Pod/Job patch access is limited to the operator's system namespace; doctor identifies denied permissions safely. Commits: [088dd85](https://github.com/inferal-oss/kimage/commit/088dd850b152c580e25e1893c416d2a4521356e1), [e5ac231](https://github.com/inferal-oss/kimage/commit/e5ac23183460904e06b11024cd8154212fbca9e5).
- Existing immutable credential views are verified without rewriting their metadata or requesting additional Secret write permissions. New views receive installation ownership at creation. Commits: [eeb6d49](https://github.com/inferal-oss/kimage/commit/eeb6d49fac4633bad8bde74c0af7a8215fe37b79).
- Resource checks compare JSON values across Helm and Rust serialization, preserving manager identity inside worker recovery templates while rejecting unexpected fields and genuine drift. Commits: [ed17184](https://github.com/inferal-oss/kimage/commit/ed1718410e6993d0af6959336e61bb2d09a1b19a), [c44d092](https://github.com/inferal-oss/kimage/commit/c44d0926c0e3ebd21fcc39ed05a4a9ee861f9891).
- Native readiness Jobs track node identity, placement and input images, and fit Kubernetes label limits for every supported installation name. Readiness checks wait through normal worker replacement without accepting ownership or specification drift. Commits: [307f8f0](https://github.com/inferal-oss/kimage/commit/307f8f0152f5f48b792086409920d53c745875fe).
- Build-log snapshots pair terminal phases with final output. Following an attempt waits through startup and bounded transient connection failures, reports gaps, and retains full attempt identities in valid diagnostic Secret names. Commits: [3674da9](https://github.com/inferal-oss/kimage/commit/3674da9f649c0eac8467c0f483c8b0cf365f6201), [03372fe](https://github.com/inferal-oss/kimage/commit/03372fe380b39a1189543e4853607a5f8dcf3ada), [7019062](https://github.com/inferal-oss/kimage/commit/701906228793087a9df1bf453e883868250733fa).
- Builds wait for a replacement worker's Pod and service endpoints before capturing inputs, avoiding empty interrupted attempts during normal worker recovery. Commits: [4409c4a](https://github.com/inferal-oss/kimage/commit/4409c4a6034d5c13a8b68877103764b4dea29e90).
- The BuildKit progress adapter accepts batched records up to 1 MiB, preventing valid output bursts from failing at the old 256 KiB record limit. Total progress stays bounded to 16 MiB and retained text to 64 KiB. Commits: [eeb6d49](https://github.com/inferal-oss/kimage/commit/eeb6d49fac4633bad8bde74c0af7a8215fe37b79).
- Candidate acceptance repeats failed-build diagnostics with fresh native Images to catch progress batching and recovery races before image publication. Commits: [9d868b4](https://github.com/inferal-oss/kimage/commit/9d868b496c53e14a81027265c8706f5cb8a5b8f1).
- Upgrade direction follows semantic version ordering, and native node lifecycle features remain enabled in later releases. Commits: [28789ba](https://github.com/inferal-oss/kimage/commit/28789ba22bafaa0ff01335ca4db9e9c6b0701a19).
- Native diagnostic acceptance tolerates independent stdout/stderr ordering. Cleanup progress and failures expose only bounded metadata; focused lifecycle diagnostics cover both architectures and installation scopes. Commits: [2a36942](https://github.com/inferal-oss/kimage/commit/2a36942f586684dde1d06b6fe96f664fe045a473), [567b50c](https://github.com/inferal-oss/kimage/commit/567b50c50b98a74c865824fe030b6d90c48000b8), [5584c57](https://github.com/inferal-oss/kimage/commit/5584c57698cfc4f5d126b2b0b265f73499289b35), [6dce5e8](https://github.com/inferal-oss/kimage/commit/6dce5e801cd96979104a0bac9aa5368389546a9c), [b2ed1cc](https://github.com/inferal-oss/kimage/commit/b2ed1ccf0841907cd6dd6e40269344c3a49821e6), [9ddf0a6](https://github.com/inferal-oss/kimage/commit/9ddf0a622e1ac91bfbec511f2acd8333ebdeb621).

### Security

- Optional candidate and focused native workflows encrypt failing Helm output to a maintainer-provided certificate; the private decryption key remains outside CI. Focused runs can reuse unchanged native component artifacts and run without capture. Native attempt summaries expose only allowlisted phase and public status codes and bounded metadata. Commits: [b02175f](https://github.com/inferal-oss/kimage/commit/b02175fc904dfb756e9df5917df85f27ef8f47ca), [f3a4ca4](https://github.com/inferal-oss/kimage/commit/f3a4ca476f8f04d4e2af722d01f04353456f7f04), [4165667](https://github.com/inferal-oss/kimage/commit/41656673a2531a64623162bed3e19d017e033100).
- Diagnostic Secrets carry the predecessor's reserved input marker, so rolling back to 0.4.1 cannot make private build output eligible as an ordinary build input. Commits: [ffe7247](https://github.com/inferal-oss/kimage/commit/ffe7247569f827c80d374a438ce33193f010d5db).
- Helm upgrade checks reject recovery history that would replay stale credential or build-policy bytes, before stopping components. Commits: [8de46e5](https://github.com/inferal-oss/kimage/commit/8de46e5c89e5699de2edae69fa8216f56013f00d).

**Commits:** [5f480cf](https://github.com/inferal-oss/kimage/commit/5f480cf78a0fe8af54d898bbb1482757d288d8f5), [03372fe](https://github.com/inferal-oss/kimage/commit/03372fe380b39a1189543e4853607a5f8dcf3ada), [8de46e5](https://github.com/inferal-oss/kimage/commit/8de46e5c89e5699de2edae69fa8216f56013f00d), [c44d092](https://github.com/inferal-oss/kimage/commit/c44d0926c0e3ebd21fcc39ed05a4a9ee861f9891), [ffe7247](https://github.com/inferal-oss/kimage/commit/ffe7247569f827c80d374a438ce33193f010d5db), [28789ba](https://github.com/inferal-oss/kimage/commit/28789ba22bafaa0ff01335ca4db9e9c6b0701a19), [38e9bfb](https://github.com/inferal-oss/kimage/commit/38e9bfbd673498fd19bf845223daa45162eedea8), [7019062](https://github.com/inferal-oss/kimage/commit/701906228793087a9df1bf453e883868250733fa), [ed17184](https://github.com/inferal-oss/kimage/commit/ed1718410e6993d0af6959336e61bb2d09a1b19a), [3674da9](https://github.com/inferal-oss/kimage/commit/3674da9f649c0eac8467c0f483c8b0cf365f6201), [b02175f](https://github.com/inferal-oss/kimage/commit/b02175fc904dfb756e9df5917df85f27ef8f47ca), [f3a4ca4](https://github.com/inferal-oss/kimage/commit/f3a4ca476f8f04d4e2af722d01f04353456f7f04), [c9ecc2d](https://github.com/inferal-oss/kimage/commit/c9ecc2d068f5a0bf85bb18ac1bde16bc858e3d10), [307f8f0](https://github.com/inferal-oss/kimage/commit/307f8f0152f5f48b792086409920d53c745875fe), [2a36942](https://github.com/inferal-oss/kimage/commit/2a36942f586684dde1d06b6fe96f664fe045a473), [567b50c](https://github.com/inferal-oss/kimage/commit/567b50c50b98a74c865824fe030b6d90c48000b8), [5584c57](https://github.com/inferal-oss/kimage/commit/5584c57698cfc4f5d126b2b0b265f73499289b35), [088dd85](https://github.com/inferal-oss/kimage/commit/088dd850b152c580e25e1893c416d2a4521356e1), [6dce5e8](https://github.com/inferal-oss/kimage/commit/6dce5e801cd96979104a0bac9aa5368389546a9c), [81140cb](https://github.com/inferal-oss/kimage/commit/81140cb0c094f65e11c3354d3c5046e085b88de9), [4cfddd3](https://github.com/inferal-oss/kimage/commit/4cfddd3ef8800bf578ab40298cb1a0a71bc33480), [e5ac231](https://github.com/inferal-oss/kimage/commit/e5ac23183460904e06b11024cd8154212fbca9e5), [9ddf0a6](https://github.com/inferal-oss/kimage/commit/9ddf0a622e1ac91bfbec511f2acd8333ebdeb621), [eeb6d49](https://github.com/inferal-oss/kimage/commit/eeb6d49fac4633bad8bde74c0af7a8215fe37b79), [b2ed1cc](https://github.com/inferal-oss/kimage/commit/b2ed1ccf0841907cd6dd6e40269344c3a49821e6), [4165667](https://github.com/inferal-oss/kimage/commit/41656673a2531a64623162bed3e19d017e033100), [4409c4a](https://github.com/inferal-oss/kimage/commit/4409c4a6034d5c13a8b68877103764b4dea29e90), [f0d1475](https://github.com/inferal-oss/kimage/commit/f0d14755f81ccdfc819d994a5e52aceb47e97ebe), [2f0abde](https://github.com/inferal-oss/kimage/commit/2f0abde3ce88dd848600b216294b0079f8593103), [b6bab31](https://github.com/inferal-oss/kimage/commit/b6bab319f041e640e33264cf7299f19f703bc6db), [9d868b4](https://github.com/inferal-oss/kimage/commit/9d868b496c53e14a81027265c8706f5cb8a5b8f1).

## [0.4.1] - 2026-10-16

### Added

- Recorded [real mixed-cluster acceptance](docs/mixed-install-results.json) for the published 0.4.0 CLI and public Helm chart: native GHCR pulls, fresh unknown/amd64/arm64 placement cases, retained resource identities and complete OCI graph readback after uninstall/reinstall. Commits: [1fa8f37](https://github.com/inferal-oss/kimage/commit/1fa8f370f26d6d94996c86df93b4b7815f9526e2).
- Repository instructions now require Keep a Changelog entries and synchronized release documentation for future changes. Commits: [ca59bab](https://github.com/inferal-oss/kimage/commit/ca59bab0001c15076e51f69b0ce7f620912027ab).

### Changed

- Updated Cargo and Helm installation examples, README, website and agent essentials for 0.4.1, which includes the CLI uninstall fix. Commits: [ca59bab](https://github.com/inferal-oss/kimage/commit/ca59bab0001c15076e51f69b0ce7f620912027ab).

### Fixed

- CLI uninstall retries status-only resource-version conflicts when Kubernetes LIST items omit the type metadata included by GET. UID, specification and ownership checks remain enforced. This fixes an intermittent `CleanupIdentityConflict` observed while testing published 0.4.0 on a real mixed cluster. Commits: [1fa8f37](https://github.com/inferal-oss/kimage/commit/1fa8f370f26d6d94996c86df93b4b7815f9526e2).
- Native installation E2E waits through transient null kubelet image reports instead of failing before installation. Regression coverage keeps the cold-cache check intact. Commits: [7ec2c81](https://github.com/inferal-oss/kimage/commit/7ec2c81e0c6e9d64b67b7d64cf5994a9d9e8f109).

**Commits:** [1fa8f37](https://github.com/inferal-oss/kimage/commit/1fa8f370f26d6d94996c86df93b4b7815f9526e2), [7ec2c81](https://github.com/inferal-oss/kimage/commit/7ec2c81e0c6e9d64b67b7d64cf5994a9d9e8f109), [ca59bab](https://github.com/inferal-oss/kimage/commit/ca59bab0001c15076e51f69b0ce7f620912027ab).

## [0.4.0] - 2026-10-16

### Added

- Native amd64/arm64 component images published to GHCR by the release workflow, with complete immutable OCI indexes and public-pull verification. Commits: [74a3026](https://github.com/inferal-oss/kimage/commit/74a302659470eb2cf8f5c1a2ad4a36f8208fde24).
- A Helm chart repository at `https://kimage.dev/charts`, containing the same release image pins and resource definitions as the CLI. Commits: [74a3026](https://github.com/inferal-oss/kimage/commit/74a302659470eb2cf8f5c1a2ad4a36f8208fde24).
- Eight native Kubernetes acceptance cases covering CLI and Helm, namespace and cluster scope, on both architectures, using fresh TLS registry pulls. Commits: [74a3026](https://github.com/inferal-oss/kimage/commit/74a302659470eb2cf8f5c1a2ad4a36f8208fde24), [ba88f0c](https://github.com/inferal-oss/kimage/commit/ba88f0c697c751b1922c0751926a5a91574db9f3).

### Changed

- `kimage install` now discovers nodes, generates credentials and applies ordinary Kubernetes resources. It fetches only a small verified image catalog; component executables and layers no longer travel through the Kubernetes API. Commits: [aec8d90](https://github.com/inferal-oss/kimage/commit/aec8d90c3a597816cae0ada37bb1b04e5b1fc511).
- Installation defaults to the current kubeconfig application namespace, a dedicated system namespace and the default StorageClass. Reapplication preserves credentials, build policy and registry node identity. Commits: [aec8d90](https://github.com/inferal-oss/kimage/commit/aec8d90c3a597816cae0ada37bb1b04e5b1fc511), [7a9f119](https://github.com/inferal-oss/kimage/commit/7a9f11914fed325a0813b7084be1c9920998eb7d).
- Releases contain four installers, the image catalog and pinned chart instead of bootstrap bundles. Legacy bootstrap commands remain available for independently retained older installations. Commits: [aec8d90](https://github.com/inferal-oss/kimage/commit/aec8d90c3a597816cae0ada37bb1b04e5b1fc511), [74a3026](https://github.com/inferal-oss/kimage/commit/74a302659470eb2cf8f5c1a2ad4a36f8208fde24).
- CLI and Helm uninstall remove disposable installation resources while retaining the registry PVC, Images, ImageReleases and shared CRDs. Commits: [aec8d90](https://github.com/inferal-oss/kimage/commit/aec8d90c3a597816cae0ada37bb1b04e5b1fc511).
- Updated installation guides, quickstarts, website and agent essentials for image-pull and Helm installation. Commits: [aec8d90](https://github.com/inferal-oss/kimage/commit/aec8d90c3a597816cae0ada37bb1b04e5b1fc511), [74a3026](https://github.com/inferal-oss/kimage/commit/74a302659470eb2cf8f5c1a2ad4a36f8208fde24).

### Fixed

- Fresh installs use an explicit serving ServiceAccount, avoiding dependence on asynchronous creation of the namespace's default account. Commits: [4d8b09e](https://github.com/inferal-oss/kimage/commit/4d8b09e2a9a4c4583f7d0b1e31494ee9a170c83f).
- Helm upgrades preserve administrator-owned build policy without taking ownership of its fields. Reapplication preserves the original policy JSON bytes. Commits: [4d8b09e](https://github.com/inferal-oss/kimage/commit/4d8b09e2a9a4c4583f7d0b1e31494ee9a170c83f), [9b831cf](https://github.com/inferal-oss/kimage/commit/9b831cf2da4ef051731be59b07d31c9bf63aa42f).

**Commits:** [aec8d90](https://github.com/inferal-oss/kimage/commit/aec8d90c3a597816cae0ada37bb1b04e5b1fc511), [74a3026](https://github.com/inferal-oss/kimage/commit/74a302659470eb2cf8f5c1a2ad4a36f8208fde24), [7a9f119](https://github.com/inferal-oss/kimage/commit/7a9f11914fed325a0813b7084be1c9920998eb7d), [4d8b09e](https://github.com/inferal-oss/kimage/commit/4d8b09e2a9a4c4583f7d0b1e31494ee9a170c83f), [9b831cf](https://github.com/inferal-oss/kimage/commit/9b831cf2da4ef051731be59b07d31c9bf63aa42f), [ba88f0c](https://github.com/inferal-oss/kimage/commit/ba88f0c697c751b1922c0751926a5a91574db9f3).

## [0.3.1] - 2026-10-15

### Fixed

- Removed the 30-second socket read timeout from the Kubernetes client. Healthy
  quiet watches and long-lived exec/port-forward streams must not expire at that
  limit. A regression test covers a delayed watch event; native installation E2E
  checks that a fresh Image is reconciled after a quiet catalog interval. Commits: [861d370](https://github.com/inferal-oss/kimage/commit/861d37040a7d34954d71f488c65b0b68151c111b).
- Release bundle verification now checks gateway/proxy against their separate
  serving version, allowing patch releases to retain serving compatibility. Commits: [d9bad15](https://github.com/inferal-oss/kimage/commit/d9bad151bd53ddd806a5328664e65784c2f62cc0).

### Added

- An explicitly enabled mixed-cluster E2E using fresh Images for unknown, amd64,
  and arm64 workload placement. It checks native builds, exact variant and demand
  sets, and native execution through the selected OCI index or manifest. Public
  CI checks the harness; the live test requires an existing mixed cluster.
  [Two recorded real-cluster runs](docs/mixed-cluster-results.json) passed using
  the published 0.3.0 binaries. Commits: [c31408c](https://github.com/inferal-oss/kimage/commit/c31408cf7fb473291ccee72d856555b278379ba1), [ae27d92](https://github.com/inferal-oss/kimage/commit/ae27d92612bfb56c9821c3fa94a40bf87ac9e295), [83bf364](https://github.com/inferal-oss/kimage/commit/83bf36402e94b30155318e460a428963b8f37ab1), [4bfe453](https://github.com/inferal-oss/kimage/commit/4bfe4537416a7c2ab5d84c3869a77975bee14e6c).

### Changed

- Clarified README and website positioning against kpack: both provide
  declarative Images; kimage integrates an included registry, immutable version
  catalog, and workload image/version requests resolved during Pod admission. Commits: [c31408c](https://github.com/inferal-oss/kimage/commit/c31408cf7fb473291ccee72d856555b278379ba1).

**Commits:** [c31408c](https://github.com/inferal-oss/kimage/commit/c31408cf7fb473291ccee72d856555b278379ba1), [ae27d92](https://github.com/inferal-oss/kimage/commit/ae27d92612bfb56c9821c3fa94a40bf87ac9e295), [83bf364](https://github.com/inferal-oss/kimage/commit/83bf36402e94b30155318e460a428963b8f37ab1), [4bfe453](https://github.com/inferal-oss/kimage/commit/4bfe4537416a7c2ab5d84c3869a77975bee14e6c), [861d370](https://github.com/inferal-oss/kimage/commit/861d37040a7d34954d71f488c65b0b68151c111b), [d453580](https://github.com/inferal-oss/kimage/commit/d453580204a92e9c9f994c6c53fe0209049e9f77), [d9bad15](https://github.com/inferal-oss/kimage/commit/d9bad151bd53ddd806a5328664e65784c2f62cc0).

## [0.3.0] - 2026-10-15

### Added

- Native Linux/amd64 and Linux/arm64 workers, architecture-specific bundles, and
  Linux/macOS installers for both architectures. Installation selects bundles
  from cluster Node architecture and registers native workers automatically. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d).
- Placement-aware image selection from node names, architecture selectors and
  required node affinity. Unconstrained mixed-cluster workloads receive verified
  multi-platform OCI indexes; every child graph and layer is checked. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d).
- Independent immutable Image UID/version/platform releases and platform status.
  Aggregate indexes require consistent input captures across native variants. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d).
- Default on-demand production for automatic Images, bounded generation-scoped
  ImageDemand records, and an administrator ConfigMap for optional eager builds.
  Dry-run requests produce no demand; explicit platforms request direct builds. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d), [4edf5e6](https://github.com/inferal-oss/kimage/commit/4edf5e6c4aff73b0bb76c83c1c98f7a7cb7ff673).
- Namespace-scoped and cluster-wide installation, runtime scope filters,
  administrator exclusions, overlap checks and same-namespace image consumption. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d).
- Native Kubernetes E2E for both scopes on amd64 and arm64, required before release
  publication. Coverage includes real builds/pulls, controller templates, build
  policy, dry-run, RBAC, namespace boundaries and retained storage on uninstall. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d), [36bf607](https://github.com/inferal-oss/kimage/commit/36bf607e0f627c710377abf3168a593634858986), [de1b161](https://github.com/inferal-oss/kimage/commit/de1b16196c981573370300e207510fe05989c645), [c08a884](https://github.com/inferal-oss/kimage/commit/c08a884fb38993ebc4670e9f40424377e3765c4c), [7a23f38](https://github.com/inferal-oss/kimage/commit/7a23f38da7c36a59c3e4bf36a3d3da544006a0ae).
- Keep a Changelog structure and CI validation for future release notes. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d).

### Changed

- Omitted Image platform now selects native platforms automatically. Explicit
  `platform` and `platforms` constrain native variants. Apply a consumer workload
  before waiting for an automatic Image under the default OnDemand policy. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d).
- Schema revision 5 accepts exact shipped revision 2/3/4 definitions, preserving
  data and rejecting drift or downgrades. Serving binaries report version 0.3.0;
  cluster scope requires that version's namespace routing support. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d).
- Interrupted registry publication retries within a bounded deadline using fresh
  upload sessions and complete byte readback. Integrity and authorization errors
  still fail immediately. Commits: [a21de0c](https://github.com/inferal-oss/kimage/commit/a21de0cfcc7fcffd8287e02162df9159b11729a6).
- Uninstall tolerates status-only resource-version races while retaining UID and
  ownership checks and refusing concurrent spec edits or replacement resources. Commits: [3ca9bb1](https://github.com/inferal-oss/kimage/commit/3ca9bb121159da8f4991490761a9450348036526).
- README, tutorial, handbook and agent essentials document platform inputs,
  selection, demand/eager policy, both scopes and automatic bundle delivery. Commits: [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d), [36bf607](https://github.com/inferal-oss/kimage/commit/36bf607e0f627c710377abf3168a593634858986).

**Commits:** [554c3b7](https://github.com/inferal-oss/kimage/commit/554c3b76571a85d479c62db1bae1a0281cd0284b), [e93b45d](https://github.com/inferal-oss/kimage/commit/e93b45db696ddf70c9c8d3a782cacb5dda3cd2ba), [53caefb](https://github.com/inferal-oss/kimage/commit/53caefb20a8522e76acc3cc95e13af43d4b450eb), [61f4038](https://github.com/inferal-oss/kimage/commit/61f40387a3b0315dadbfc998aa2a9c196df0691d), [36bf607](https://github.com/inferal-oss/kimage/commit/36bf607e0f627c710377abf3168a593634858986), [de1b161](https://github.com/inferal-oss/kimage/commit/de1b16196c981573370300e207510fe05989c645), [4edf5e6](https://github.com/inferal-oss/kimage/commit/4edf5e6c4aff73b0bb76c83c1c98f7a7cb7ff673), [c08a884](https://github.com/inferal-oss/kimage/commit/c08a884fb38993ebc4670e9f40424377e3765c4c), [7a23f38](https://github.com/inferal-oss/kimage/commit/7a23f38da7c36a59c3e4bf36a3d3da544006a0ae), [2665565](https://github.com/inferal-oss/kimage/commit/2665565399f070aeb86629ca9839092a13f1356e), [a21de0c](https://github.com/inferal-oss/kimage/commit/a21de0cfcc7fcffd8287e02162df9159b11729a6), [3ca9bb1](https://github.com/inferal-oss/kimage/commit/3ca9bb121159da8f4991490761a9450348036526).

## [0.2.0] - 2026-10-15

### Added

- Official GitHub Actions builds now produce a complete Linux/amd64 component
  bundle, its inventory and checksums, and native Linux/amd64 and macOS/arm64
  installers as GitHub release assets. Administrators no longer need to package
  their own bundle for the supported profile. Commits: [ff89753](https://github.com/inferal-oss/kimage/commit/ff89753a0a50e9127f13aeee0346f361c21b05d2).
- `kimage install` runs the standard setup phases in one resumable command. It
  validates all credential directories before bootstrap writes and avoids
  repeating bootstrap after a partial handover. `kimage-install` remains a
  compatible alias with the same commands. Commits: [68c0540](https://github.com/inferal-oss/kimage/commit/68c0540f3100387f7f33ac93d83c00213b17ddbc).
- The installer automatically fetches the exact official release matching its
  package version, verifies GitHub asset digests, inventory, static executables,
  and complete OCI graphs, and caches it before Kubernetes access. Commits: [ff89753](https://github.com/inferal-oss/kimage/commit/ff89753a0a50e9127f13aeee0346f361c21b05d2).
- `bundle-fetch` prepares an installation or air-gap transfer without kubeconfig;
  `--release`, `--bundle-cache`, and `--offline` make selection and reuse explicit.
  Local `--bundle` with an independently trusted inventory remains supported. Commits: [ff89753](https://github.com/inferal-oss/kimage/commit/ff89753a0a50e9127f13aeee0346f361c21b05d2).
- Atomic cache installation, bounded archive extraction, process locking, and
  regression coverage for corrupt, interrupted, unsafe, and incomplete downloads. Commits: [ff89753](https://github.com/inferal-oss/kimage/commit/ff89753a0a50e9127f13aeee0346f361c21b05d2).
- Release publishing stages verified draft assets, publishes crates through OIDC
  with safe partial-release resume, then publishes GitHub assets and verifies the
  real public fetch and offline path. Commits: [ff89753](https://github.com/inferal-oss/kimage/commit/ff89753a0a50e9127f13aeee0346f361c21b05d2), [e537b4a](https://github.com/inferal-oss/kimage/commit/e537b4ab38e3035ef2d48d18fdb145b3f8133c35), [554c3b7](https://github.com/inferal-oss/kimage/commit/554c3b76571a85d479c62db1bae1a0281cd0284b), [e93b45d](https://github.com/inferal-oss/kimage/commit/e93b45db696ddf70c9c8d3a782cacb5dda3cd2ba).
- Updated handbook, README, quickstart, and self-contained agent guidance. Commits: [ff89753](https://github.com/inferal-oss/kimage/commit/ff89753a0a50e9127f13aeee0346f361c21b05d2), [68c0540](https://github.com/inferal-oss/kimage/commit/68c0540f3100387f7f33ac93d83c00213b17ddbc).

```sh
cargo install kimage-control --version 0.2.0 --locked
kimage install --help
```

The supported Kubernetes/runtime, persistent storage, cached carrier, issuer
credentials, and user namespace prerequisites still apply. Automatic fetching
changes bundle delivery; it does not broaden the serving-only upgrade window or
provide an operator/schema downgrade path. See the
[handbook](https://kimage.dev/docs/).

**Commits:** [c89b58c](https://github.com/inferal-oss/kimage/commit/c89b58c4c8dbbebaef24eb3ea2aba90622d58b39), [c26ee91](https://github.com/inferal-oss/kimage/commit/c26ee9140c77b3ceda5992e99042adc4a2961f97), [ff89753](https://github.com/inferal-oss/kimage/commit/ff89753a0a50e9127f13aeee0346f361c21b05d2), [68c0540](https://github.com/inferal-oss/kimage/commit/68c0540f3100387f7f33ac93d83c00213b17ddbc), [e537b4a](https://github.com/inferal-oss/kimage/commit/e537b4ab38e3035ef2d48d18fdb145b3f8133c35).

## [0.1.0] - 2026-10-15

### Added

Initial alpha release for the documented Linux/amd64 Kubernetes runtime profile.

- Native `kimage-install` command for API-only bootstrap, verification, recovery,
  and compatible serving upgrades and rollback. Commits: [a048539](https://github.com/inferal-oss/kimage/commit/a048539ea52b915110966f9d298314d32dfbdd0b), [6d6106e](https://github.com/inferal-oss/kimage/commit/6d6106e50ca1893e2f0cfa91537c8a2a5ad61fac).
- Declarative Image production through pinned BuildKit, verified OCI publication,
  and retained immutable ImageRelease resources. Commits: [a048539](https://github.com/inferal-oss/kimage/commit/a048539ea52b915110966f9d298314d32dfbdd0b).
- Exact, channel, and readable colon/query range selectors with digest-pinned Pod
  admission and ordinary kubelet pulls through node-local proxies. Commits: [a048539](https://github.com/inferal-oss/kimage/commit/a048539ea52b915110966f9d298314d32dfbdd0b).
- Separate retained-registry storage and disposable worker cache, two-node
  delivery/recovery, and an optional independent registry exporter. Commits: [a048539](https://github.com/inferal-oss/kimage/commit/a048539ea52b915110966f9d298314d32dfbdd0b).
- Operational metrics, offline bundle tooling, documented live acceptance
  evidence, and macOS/Linux test coverage. Commits: [a048539](https://github.com/inferal-oss/kimage/commit/a048539ea52b915110966f9d298314d32dfbdd0b), [6d4121e](https://github.com/inferal-oss/kimage/commit/6d4121eaa78cbc3efe6350dd8f125704ece8b992), [6d6106e](https://github.com/inferal-oss/kimage/commit/6d6106e50ca1893e2f0cfa91537c8a2a5ad61fac).

Install the native commands with Rust 1.99.0 or newer:

```sh
cargo install kimage-control --version 0.1.0 --locked
kimage-install --help
```

Cluster installation requires a trusted Linux/amd64 component bundle, TLS
credentials, and a supported runtime profile. Builds initially support scratch
images and provisioned local inputs. See the
[compatibility limits](README.md#compatibility-and-limits).

**Commits:** [5ca2d3b](https://github.com/inferal-oss/kimage/commit/5ca2d3b557e8d48786dc895ce7f1d8f56b9f9fb8), [d5ad7cb](https://github.com/inferal-oss/kimage/commit/d5ad7cb2e36ea9c030efa3d55bc3a84db40eee9d), [a048539](https://github.com/inferal-oss/kimage/commit/a048539ea52b915110966f9d298314d32dfbdd0b), [6d4121e](https://github.com/inferal-oss/kimage/commit/6d4121eaa78cbc3efe6350dd8f125704ece8b992), [d6dc671](https://github.com/inferal-oss/kimage/commit/d6dc6716ae445c20da9f9d3f2d72af67f3c99253), [a2b618a](https://github.com/inferal-oss/kimage/commit/a2b618a81fd4089faad0a3347b5ecf245c8f0604), [caec4d9](https://github.com/inferal-oss/kimage/commit/caec4d9993a92fc36943b2e94760480c85aa1b95), [6e2f9df](https://github.com/inferal-oss/kimage/commit/6e2f9df6dc262831bffb80f5259d10db3fca16b3), [6d6106e](https://github.com/inferal-oss/kimage/commit/6d6106e50ca1893e2f0cfa91537c8a2a5ad61fac).

[Unreleased]: https://github.com/inferal-oss/kimage/compare/v0.6.1...HEAD
[0.6.1]: https://github.com/inferal-oss/kimage/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/inferal-oss/kimage/compare/v0.5.2...v0.6.0
[0.5.2]: https://github.com/inferal-oss/kimage/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/inferal-oss/kimage/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/inferal-oss/kimage/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/inferal-oss/kimage/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/inferal-oss/kimage/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/inferal-oss/kimage/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/inferal-oss/kimage/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/inferal-oss/kimage/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/inferal-oss/kimage/releases/tag/v0.1.0
