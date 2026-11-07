# Branch acceptance checks

Close the outstanding storage, GUI, remote-filesystem, and performance gaps using an isolated profile and temporary fixtures. Never operate on the user's real recents store, trash contents, desktop session, or remote mounts.

1. Compile the current application and a storage acceptance example. Mount a temporary directory over the profile in a private filesystem namespace, preserving the session's environment. Exercise the production recents writer against real XBEL storage and the application's real recents/trash scans. Verify trash restoration preserves bytes.
2. Run the current application on a private Xvfb display and D-Bus session. Exercise local listing, list/grid scrolling, navigation, menus, image details, and live filesystem refreshes. Retain screenshots and explicit assertions; distinguish visual checks from timing measurements.
3. Run a private loopback remote-filesystem server and a private GVFS session. Exercise actual remote directory enumeration, directory/file classification, nested navigation, and refresh after mutation. The remote setup worker owns scratch artifacts only; root owns source and builds.
4. Measure the frame pipeline across small and large directories in a deliberate non-functional run. Record environment, sample counts, distributions, and bounds of the conclusion. Run timing-sensitive work exclusively rather than adding timing assertions to the normal gate.
5. Review the added harnesses, run their acceptance checks, and run the canonical three-configuration functional matrix if source/test additions require it. Update the review report with measured evidence and any remaining environmental limitations.

Storage and GUI work share the application build and run serially. Remote server setup can proceed independently in scratch storage. A private display avoids interacting with the user's desktop. A loopback server gives reproducible protocol coverage without requesting credentials; it does not establish WAN latency or every remote backend's behavior.
