# Project memory

- [Background work and acceptance](docs/memory/background-work-and-acceptance.md): request ownership, persistence ordering, metadata provenance and isolated validation.
- [Upstream contributions](docs/memory/upstream-contributions.md): no PRs or issues upstream; fixes stay in the fork, each with a README line linked to its issue.

# Install after merge

After every change lands on `master` (`/sqmerge`, `/sync-upstream`), build the RPM from the main
checkout without waiting to be asked. The build takes several minutes, so launch it detached and
wait with the watcher `arm-run` prints:

```sh
git -C /mnt/work/projects/cosmic-de/cosmic-files fetch upstream 'refs/tags/epoch-*:refs/tags/epoch-*'
arm-run -m 20G -l cosmic-files-rpm -C /mnt/work/projects/cosmic-de/cosmic-files -- /mnt/work/projects/cosmic-de/cosmic-files/packaging/build-rpm.sh
```

Installing needs root, so it is the user's step: report it as not live until they run
`sudo dnf5 install /mnt/work/projects/cosmic-de/cosmic-files/target/rpm/cosmic-files-local.rpm`,
then close and reopen Files. Details: [local build and install](docs/guides/local-build-and-install.md).
