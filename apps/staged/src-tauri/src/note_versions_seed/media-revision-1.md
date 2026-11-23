# Settings redesign: review notes

The settings page was rebuilt around a single column. These notes record what the review found before the old page was removed.

## The old page

The old layout put the account controls in a sidebar, beside the preferences.

![The settings page before the redesign](staged-media://5d0c3b7a-2f41-4e68-9a57-1c0e8b6d4f21.png)

Opening it took two clicks from the home screen, the second on a menu that closed on its own.

![Opening settings from the home screen](staged-media://8a4f6e12-7b93-4c05-b1d8-3e9a5c7f2064.mp4)

## Findings

- The sidebar hid the sign-out button in a narrow window.
- Saving gave no confirmation, so people saved twice.

## Decision

The single column ships behind the `settings-v2` flag, and the sidebar goes once the flag is on for everyone.
