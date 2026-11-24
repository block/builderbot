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
 * keyword, but only while `autocomplete` is `off` or absent. On the iOS
 * simulator, `name="search"` with `autocomplete="off"` removed the offers.
 * The rest was read from the script Safari injects into the page and not
 * tried on a device: that an absent `autocomplete` does as well as `off`,
 * and that any other token, including a made-up one, switches the exemption
 * off.
 *
 * So the two attributes belong together, and a caller must not override
 * either. There may be a second reason. Browsers are known to keep a form
 * history, which offers what was typed into a field before, and to key it by
 * `name`. If Safari does that here, inputs that share `name="search"` could
 * offer each other's old values, and `autocomplete="off"` is the usual way
 * to opt a field out. None of this was run. No input was tried with the
 * `name` and without `autocomplete="off"`, so it is not known that such an
 * offer would appear, or that `off` is what prevents it.
 *
 * The types cannot stop a call site writing its own `name` or
 * `autocomplete` beside the spread, so safariAutofill.guard.test.ts reads
 * every call site's source for one.
 *
 * `name="search"` is a workaround keyed to Safari's keyword list, which Apple
 * can change. It lives here so that there is one place to change with it. The
 * `name` has no other effect as long as the form is handled in JS and nothing
 * reads its controls by name. The Return key keeps its "return" label.
 *
 * It does not stop the Passwords offer on a `type="password"` input: Safari
 * offers saved passwords there whatever the `name`, and keeps doing so after
 * the type is switched to `text`. A password field beside a text field still
 * needs `name="search"`, though. That was seen on an isolated copy of the
 * env var row, not in the app: with only the name field edited, the name
 * field kept its "Sign In" sheet, and adding `name="search"` to the value
 * field as well removed it. The real app was run only with both inputs
 * edited, where the name field got no offer.
 */
export const NO_SAFARI_AUTOFILL = {
  name: 'search',
  autocomplete: 'off',
} as const satisfies HTMLInputAttributes;
