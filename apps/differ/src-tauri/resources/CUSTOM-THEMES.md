# Custom Themes

Differ supports custom syntax themes using the VS Code / Shiki theme JSON format.

## Adding a custom theme

1. Create the themes directory:
   ```
   mkdir -p ~/.differ/themes
   ```

2. Copy the included template as a starting point:
   ```
   cp theme-template.json ~/.differ/themes/my-theme.json
   ```

3. Edit the JSON file. The format follows the [VS Code color theme spec](https://code.visualstudio.com/api/references/theme-color):
   - `name` — theme identifier (shown in the theme picker)
   - `type` — `"dark"` or `"light"` (auto-detected from `editor.background` if omitted)
   - `colors` — editor colors (`editor.background`, `editor.foreground`, git decoration colors, etc.)
   - `settings` — TextMate token color rules with `scope` and `foreground`/`fontStyle`

4. Restart Differ. Custom themes appear in the theme picker alongside built-in themes.

## Notes

- Files prefixed with `_` (e.g. `_draft.json`) are ignored.
- Theme appearance (light/dark) is auto-detected from `editor.background` luminance.
- Any valid VS Code `.json` theme file should work — you can drop in themes from the [VS Code marketplace](https://marketplace.visualstudio.com/) or other sources.
