# Branch review fixes

The accepted review calls for six repairs while preserving responsive directory browsing.

1. Every app and picker scan has an identity. An obsolete completion cannot replace a listing or release a newer scan's pending work. Watcher bursts remain coalesced; failed workers release their own request.
2. Recents record, clear, and remove mutations execute in submission order, off the GUI thread. One failed mutation must not stop subsequent work.
3. Trash scans publish only the latest requested result. A delayed completion cannot overwrite a newer empty/full state.
4. Local image details acquire dimensions from accepted thumbnail work. Stale thumbnail work cannot populate a replacement item; scanning does not add image-header reads.
5. Launcher menus require a regular desktop-entry candidate and a successful parse. Directories, failed parses, and pending parses retain ordinary file actions.
6. Sidebar menus distinguish unknown metadata from files and directories. Type-dependent actions wait for metadata; type-independent actions remain available.

Validation uses deterministic completion ordering, temporary filesystem fixtures, and the repository's three CI feature configurations. User recents, trash, and deployed binaries must remain untouched by tests.

Request IDs preserve navigation responsiveness when an older mount is slow; serializing every scan would make the new location wait. Recents use a FIFO worker because all mutations rewrite one store and their order is meaningful. Menu loading uses ordinary/type-independent actions rather than guessing a file type. Thumbnail dimensions are reused rather than eagerly reading every image.
