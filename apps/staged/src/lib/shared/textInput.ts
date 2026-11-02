// Input types that open no keyboard (buttons, toggles, pickers iOS draws in
// its own popover).
const NON_TEXT_INPUT_TYPES = new Set([
  'button',
  'checkbox',
  'color',
  'file',
  'hidden',
  'image',
  'radio',
  'range',
  'reset',
  'submit',
]);

/** Whether `el` takes typing, i.e. focusing it can raise an on-screen keyboard. */
export function isTextInput(el: Element | null): boolean {
  if (!el) return false;
  if (el.tagName === 'TEXTAREA') return true;
  if (el.tagName === 'INPUT') return !NON_TEXT_INPUT_TYPES.has((el as HTMLInputElement).type);
  return (el as HTMLElement).isContentEditable === true;
}
