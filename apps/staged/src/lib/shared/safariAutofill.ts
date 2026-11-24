import type { HTMLInputAttributes } from 'svelte/elements';

/**
 * Attributes that stop Safari offering AutoFill on a text input that is not
 * for personal details. Spread onto the input: `<Input {...NO_SAFARI_AUTOFILL} />`.
 *
 * Safari decides what a field is for from its label, `id`, `name`,
 * placeholder, `aria-label` and nearby text, and ignores `autocomplete="off"`
 * when it does. A field labelled "Name" is offered the user's contact name.
 * A lone field in a form whose placeholder mentions "login" is treated as a
 * username: a "Passwords" key above the keyboard, or a "Sign In" sheet in
 * place of the keyboard once a login is saved for the host.
 *
 * Safari exempts a field when one of those sources matches its `search`
 * keyword, but only while `autocomplete` is `off` or absent. Any other
 * token, including a made-up one, switches the exemption off. So the two
 * attributes belong together, and a caller must not override either.
 *
 * `name="search"` is a workaround keyed to Safari's keyword list, which Apple
 * can change. It lives here so that there is one place to change with it. The
 * `name` has no other effect as long as the form is handled in JS and nothing
 * reads its controls by name. The Return key keeps its "return" label.
 *
 * It does not stop the Passwords offer on a `type="password"` input: Safari
 * offers saved passwords there whatever the `name`, and keeps doing so after
 * the type is switched to `text`. A password field beside a text field still
 * needs these attributes, though: in the env var row, the name field kept
 * its "Sign In" sheet until the value field carried them as well.
 */
export const NO_SAFARI_AUTOFILL = {
  name: 'search',
  autocomplete: 'off',
} as const satisfies HTMLInputAttributes;
