# Release build

The canonical `just build-release --locked --offline -j12` recipe passed for source `2ab787e0e04b85e8ca3d2c6bc7f1f6ec204ea8fc`. It built the application, desktop applet, and executable thumbnailer. The [complete recipe log](artefacts/scroll-acceptance/release-bundle-build.log) and [binary manifest](artefacts/scroll-acceptance/release-manifest.json) retain the evidence.

| Binary | Bytes | SHA-256 |
| --- | ---: | --- |
| `cosmic-files` | 66037744 | `ec6b388ca34defa12ef9d75503f22292d1e44c2507f0c6be4e44d6e1b1af5977` |
| `cosmic-files-applet` | 51772728 | `10b9d53f834d87be707eed32a3a8814c9e5152f3c8eeb5d3d9a6a4418b344c85` |
| `cosmic-files-thumbnailer` | 6376384 | `16cbef254bf09a2fda091e5490bbc118c1d198ba2b70de18aae8e9eaddf065f0` |

Binaries are available under `/mnt/logs/temp/cosmic-files-review-target/release/`. No installation, deployment, or push was performed. Release GUI acceptance is tracked separately in [the acceptance report](2026-09-22-branch-acceptance.md).

An earlier gate failed because persistence draining used a Tokio macro enabled only by dev dependencies. The production-available futures macro corrected that dependency boundary; the complete test matrix and release recipe then passed. Earlier two-binary release results are superseded by this three-binary bundle.
