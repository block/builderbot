# Settings redesign: review notes

The settings page was rebuilt around a single column. These notes record what the review found before the old page was removed.

## The old page

The old layout put the account controls in a sidebar, beside the preferences.

The screenshot of it was taken down with the page: the sidebar is in the design archive.

Opening it took two clicks from the home screen, the second on a menu that closed on its own.

## Findings

- The sidebar hid the sign-out button in a narrow window.
- Saving gave no confirmation, so people saved twice.
- The search field lost its text when the window was resized.

## Decision

The single column ships behind the `settings-v2` flag, and the sidebar goes once the flag is on for everyone.
