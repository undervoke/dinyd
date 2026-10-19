# Installation and project versions

One native executable keeps runtime selection and commands on the same release lifecycle.
The Git root's `.dinyd-version` pins the project's behavior across clones and worktrees.
`update` changes the invoked installation; an explicit `init --version` changes the project pin and its matching host settings.
A cached runtime keeps its version identity, so it is not a self-update destination.
Cache identity includes the platform as well as the version, because native and Rosetta installations can coexist in one account.
Development execution must exercise the local build rather than delegate to the project's pinned release.

Init preserves an existing pin during ordinary setup, so a fresh clone does not acquire an unrelated version change.
The selected version owns both the pin and the generated adapters; resolving `latest` once keeps them aligned.
Runtime acquisition and settings validation precede project writes, so installed hooks have a prepared runtime.
Init asks which [agent hosts](lock.md#host-boundary) to configure, defaulting to existing DinyD registrations rather than inferring consent from other agent settings.
Unattended setup preserves that selection; a first unattended setup records only the pin.
Installed DinyD registrations are the host record, and init replaces or removes only the settings it owns.
Settings become active in a host that can resolve `dinyd` on PATH and has completed its required restart and trust steps.
The consuming project owns Git-hook and CI wiring.

Guard runtime selection follows the session's worktree: a host may run its hook in the main checkout while the agent edits another worktree.
One selected project runtime judges the whole tool call, including a patch that touches several files.
Guard startup uses an installed runtime and denies missing setup with an init instruction, keeping downloads outside the pre-execution path.
[Distribution](distribution.md) owns bootstrap and binary acquisition; [document operations](apply-digest.md) own evidence changes.

@import "installation.mermaid"
