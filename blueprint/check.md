# Check

The [README](../README.md#check) owns the views, scope, verdicts, and output.
`--index` honors `GIT_INDEX_FILE`, so a hook using a temporary index judges that index.
check takes no file list from its caller and reads documents and targets from the chosen view itself, so another gate tool's file scope or diff filter never narrows what it judges.

@import "check.mermaid"
