---
applyTo: "**/*.md"
---

# Markdown Instructions

## Headings

- Use headings to structure your content. Use `#` for top-level headings, `##`
  for subheadings, and so on. Keep headings concise and descriptive.

## Lists

- Use bullet points for unordered lists and numbers for ordered lists.
- Indent nested lists with two spaces.
- Any list more than 3 items long should be a numbered list

## Tables

- Ensure table header dividers are written with spaces either side of the bar
  `|`. E.g.

```markdown
| Column 1 | Column 2 |
| --- | --- |
| Data 1 | Data 2 |
```

- Note the space before and after the `|` in the header divider line.
- It is important that you don't write '|---|---|' without spaces, as that will
  appear as errors in the markdown linter. The correct format is '| --- | --- |'
  with spaces around the dashes (or around the bar symbols, if you want to
  discuss it from that perspective).

## Mermaid

1. Mermaid is used to communicate models and graphs in the markdown
2. Never generate ascii models, always use mermaid
3. Mermaid can also display a large number of graphs if needed. We currently use
   it for radar graphs in some content.
4. Every `mermaid` code fence must open with the standard dark-theme init
   directive, as the first line inside the fence, before any diagram content:

```text
%%{init: {'theme': 'base', 'themeVariables': {'background': '#1e1e1e', 'primaryColor': '#2d2d2d', 'primaryTextColor': '#e6e6e6', 'primaryBorderColor': '#6b6b6b', 'lineColor': '#e6e6e6', 'edgeLabelBackground': '#1e1e1e', 'tertiaryColor': '#2d2d2d'}}}%%
```

- This tunes the diagram for the dark editor/GitHub preview background.
  Renderers that need a light background (PDF exports, the website) strip this
  directive themselves rather than the markdown source being maintained in two
  variants.


## Code Blocks

- Code blocks follow the formatting used in the other formatting instructions in
  the .github/instructions/ folder.
- Code blocks should always have a language specified as part of the fences

## ToC

- For longer markdown files, include a table of contents at the top using
  stadard syntax
- Use the standard markdown link syntax with anchors to the headings.

## Checklists

1. Use checklists for task lists.
2. Use `- [ ]` for not yet started items
3. Use `- [-]` for in progress items.
4. Use `- [x]` for completed items.
5. Indent nested checklists with two spaces.

## General

- We use github markdown
- We render markdown with a dark background, but can render the files as pdf
  which require a white background
- So sometimes 'themes' are applied to the markdown or mermaid
