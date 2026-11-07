#!/usr/bin/python3 -I
"""fmt-stage.py <worktree>: rustfmt a staged tree, then put back upstream's one unformatted hunk (app.rs,
ItemMetadata::Trash in the recently-trashed rescan) so the series never reformats upstream's own code."""
import subprocess,sys
wt=sys.argv[1]
subprocess.run(['cargo','fmt','--all'],cwd=wt,check=True)
I=' '*36
fmt=(f"{I}if let ItemMetadata::Trash {{ ref entry, .. }} = item.metadata\n{I}    && &entry.original_path == path\n"
     f"{I}{{\n{I}    paths.push(entry.clone());\n{I}}}\n")
up=(f"{I}if let ItemMetadata::Trash {{ ref entry, .. }} = item.metadata\n{I}    && &entry.original_path == path {{\n"
    f"{I}        paths.push(entry.clone());\n{I}    }}\n")
p=wt+'/src/app.rs'; t=open(p).read()
n=t.count(fmt)
if n!=1: sys.exit(f'fmt-stage: expected the upstream hunk once in app.rs, found {n}')
open(p,'w').write(t.replace(fmt,up))
