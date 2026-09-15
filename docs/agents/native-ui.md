# Native UI iteration

Use this workflow for GPUI styling, layout, icon work, and visual feedback.

## Diagnose before styling

1. Inspect the surrounding UI code and the component implementation before editing.
2. Work from full-context visual evidence. Follow the repository screenshot policy: ask the user to provide it; never capture their system yourself.
3. State the visible problem and the layout relationship that should replace it. If either is ambiguous, ask before changing code.

Prefer component-owned composition seams over reproducing component geometry. For controls beside tabs, use `TabBar::prefix` or `TabBar::suffix` rather than sibling elements that copy the tab bar's height, background, or borders.

## GPUI sizing

`Sizable::with_size` selects a component's semantic size and may also derive internal icon or spacing metrics. `Styled` methods such as `.w()`, `.h()`, and `.size()` refine rendered bounds. Do not assume a custom `Size::Size` value becomes every internal dimension; inspect the component's rendering code when alignment depends on it.

## Iterate

1. Change one visual hypothesis at a time.
2. Run the narrowest compile or targeted test that covers the changed code.
3. Ask the user to verify native appearance and interaction.
4. Repeat from the diagnosis if the result is rejected; do not stack speculative chrome onto an unverified design.

## Complete

After visual acceptance, run `./scripts/check`. If any subsequent edit changes production code or tests, rerun the full gate. Report automated validation and native visual validation separately.
