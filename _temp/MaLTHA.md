# MaLTHA: Markdown and Layout to HTML Assembler

## Project Overview

MaLTHA is a Python static site generator that transforms structured content files into a complete static website suitable for deployment on GitHub Pages or any static hosting platform. It processes Markdown and HTML content with TOML front matter through a three-stage pipeline -- Formator, Convertor, Generator -- producing final HTML output in a `docs/` directory.

MaLTHA is designed for small-to-medium content sites: blogs, event pages, organizational portals, documentation hubs, or any project where posts are organized by category and pages provide supplementary navigation. It has no runtime JavaScript dependency for core functionality, generates clean URL structures with multiple alias paths per resource, and produces an RSS/Atom feed automatically.

The system uses a custom file format called **ToMH** (TOML + Markup + HTML) that combines TOML metadata, Markdown/HTML content, and named template fragments in a single file. Templates are rendered through Python's `str.format()` with multiple passes, allowing layouts to compose include fragments, page content, and format strings into final output.

---

## Architecture

### Pipeline Overview

MaLTHA runs six sequential steps, orchestrated by `__main__.py`:

| Step | Action | Module |
|------|--------|--------|
| 1 | Delete `docs/` directory | Shell (`rm -r`) |
| 2 | Copy `static_files/` to `docs/` | Shell (`cp -r`) |
| 3 | Delete `mid_files/` directory | Shell (`rm -r`) |
| 4 | Parse templates and config | `database.py` -- `Formator` |
| 5 | Convert posts and pages to JSON | `convert.py` -- `Convertor` |
| 6 | Generate final HTML from JSON | `generate.py` -- `Generator` |

Steps 1-2 can be skipped with the `--skip` flag (useful when iterating on content without changing static assets). Step 3 always runs.

### Module 1: Formator (`database.py`)

The `Formator` class is responsible for loading site configuration and parsing all template files into an in-memory dictionary called `self.structure`.

**Initialization:**
- Reads `config.toml` into `self.base` using `rtoml`.

**`parse(input_str)` method:**
- Splits the input on `<!--break ... content-->` delimiters.
- Each delimiter carries key-value attributes (e.g., `type:header`, `format:md`, `title:sidebar`).
- Produces a dictionary with keys like `"header"` (parsed TOML), `"content"` (rendered HTML), and named section types (`"include"`, `"layout"`, `"format"`, `"frame"`).
- Markdown content (sections with `format:md`) is converted to HTML via `markdown2` with `fenced-code-blocks` and `tables` extras.

**`load()` method:**
- Iterates over `include_files/`, `layout_files/`, and `page_files/` (files matching `*.*ml`).
- For each parsed file, merges sections of type `include`, `layout`, and `format` into `self.structure` with prefixed keys. For example, a section `<!--break type:layout title:default content-->` becomes `self.structure["layout_default"]`.
- **Important:** Sections of type `frame` are NOT merged into `self.structure`. Frames are local to the file that defines them and are consumed during the Convertor stage.

**`oneline(input_str)` method:**
- Strips all newline characters and all 4-space indentation sequences from a string. This compresses template fragments into single-line strings. Be aware that this transformation is destructive -- content relying on whitespace formatting (e.g., indented code blocks in Markdown) should be placed in `type:content` sections, not `type:format` sections.

### Module 2: Convertor (`convert.py`)

The `Convertor` class transforms parsed data into five JSON intermediate files under `mid_files/`.

**Constructor parameters:**
- `fmt`: A `Formator` instance (provides `structure` and `base` dictionaries).
- `bu_b`: Boolean controlling base URL behavior. When `False` (debug mode), `base_url` is set to an empty string, producing relative URLs.

**Processing methods (called in order):**

1. **`post()`** -- Scans all top-level directories that are not hidden (no `.` or `_` prefix), not `*_files`, not `docs`, and not `run`. Parses each `.md` or `.html` file found. For each post:
   - Extracts TOML header fields (`title`, `date`, `short`, `categories`, etc.)
   - Generates multiple URL aliases (category+date+short, category+short, date+short, short-only, `/post/short/`)
   - Splits content on the `separator_preview` string (default `<!--excerpt-->`) to produce preview vs. full content
   - Builds category membership data

2. **`category()`** -- Aggregates category data from all posts, rendering member lists using format templates.

3. **`relate()`** -- For each post, finds other posts sharing the same categories and selects up to 3 most recent as related posts.

4. **`atom()`** -- Builds the RSS feed content and the full post listing (used by the index page and pagination). Posts are ordered in reverse (newest first). Uses `format_post_container_full` or `format_post_container_preview` depending on whether the post has an excerpt separator.

5. **`page()`** -- Processes all files in `page_files/`. Handles three content resolution strategies:
   - Direct `type:content` section in the file
   - A `frame` key in the header pointing to a named `type:frame` section
   - A `layout` key in the header pointing to a named `type:frame` section (used as `layout_content`)

**Output (`output()` method):**

| File | Contents |
|------|----------|
| `mid_files/base.json` | Merged config + computed site-wide data (post lists, category lists, page nav) |
| `mid_files/post.json` | Array of post dictionaries |
| `mid_files/post_pos.json` | Map of `short_canonical` to array index |
| `mid_files/categories.json` | Category dictionaries with member lists |
| `mid_files/page.json` | Page dictionaries keyed by title |

### Module 3: Generator (`generate.py`)

The `Generator` class reads the JSON intermediates and produces final HTML files in `docs/`.

**Rendering strategy:**

For posts and categories, the rendering follows this pattern:

```
base_dict = structure + base_info + item_dict
result = layout_default.format(**base_dict).format(**base_dict)
result = result.replace("{{", "{").replace("}}", "}")
```

The double `.format()` call is intentional: the first pass resolves top-level placeholders (e.g., `{include_head}` expands to the head template which itself contains `{title}`, `{base_url}`, etc.), and the second pass resolves the placeholders introduced by the first expansion.

For pages with `frame` or `layout` keys in their header, a third `.format()` pass is applied to resolve any remaining placeholders from the frame/layout content.

After all format passes, `{{` and `}}` are replaced with literal `{` and `}`. This is the escaping mechanism for any literal braces needed in the final HTML output.

**Generation methods:**

1. **`post()`** -- Renders each post using `layout_post` inside `layout_default`. Creates directories for all URL aliases. Each alias path gets its own `index.html`.

2. **`page()`** -- Renders each page. Pages with a `base` key use their own frame as the outermost template (bypassing `layout_default`). Pages without `base` use `layout_page` inside `layout_default`. Active sidebar navigation highlighting is applied by replacing the normal sidebar entry with an active variant.

3. **`category()`** -- Renders each category page using `layout_category` inside `layout_default`.

4. **`pagination()`** -- Only runs if `paginate_format` is non-empty. Splits the post member list into pages of `paginate_number` items each. Page 1 is written to both `docs/` (as `index.html`) and the paginated URL. Older/newer navigation buttons are rendered as active links or frozen (disabled) spans.

### Data Flow Diagram

```
config.toml ──┐
               ├──> Formator.load() ──> self.structure (templates)
include_files/ │                        self.base (config)
layout_files/  │                              │
page_files/  ──┘                              │
                                              v
posts/*/*.md ──────────────────────> Convertor ──> mid_files/*.json
posts/*/*.html ─────────────────────/                    │
                                                         v
                                                  Generator ──> docs/*.html
```

---

## Directory Structure

```
project-root/
├── config.toml                # Site-wide configuration
├── requirements.txt           # Python dependencies (includes MaLTHA)
│
├── include_files/             # Reusable HTML fragments
│   ├── head.html              # <head> element with meta tags
│   └── sidebar.html           # Navigation sidebar
│
├── layout_files/              # Layout templates
│   ├── default.html           # Outermost HTML wrapper
│   ├── post.html              # Single post layout + related posts format
│   ├── page.html              # Standalone page layout
│   ├── category.html          # Category listing layout
│   └── pagination.html        # Paginated index layout
│
├── page_files/                # Standalone pages (sorted alphabetically)
│   ├── 01-index.html          # Home page / post listing with format defs
│   ├── 02-cat.html            # Categories overview page
│   ├── 03-atom.xml            # RSS/Atom feed
│   └── 404.html               # Custom 404 page
│
├── posts/                     # Post content (or any top-level dir)
│   ├── 01-event.md
│   └── 02-event.md
│
├── static_files/              # Static assets (copied verbatim to docs/)
│   ├── main.css
│   ├── style.xsl
│   └── res/                   # Images and other resources
│
├── mid_files/                 # Generated: intermediate JSON (not in git)
├── docs/                      # Generated: final static site (deployment target)
│
├── .github/workflows/
│   └── pages.yml              # GitHub Actions deployment workflow
│
└── CLAUDE.md                  # Development instructions
```

### Post Directory Convention

Posts do not need to live in a directory named `posts/`. MaLTHA scans **all top-level directories** that satisfy these conditions:

- Is a directory (not a file)
- Name does not start with `.` or `_`
- Name does not contain `_files`
- Name is not `docs` or `run`

This means you can organize posts into multiple top-level folders (e.g., `blog/`, `news/`, `events/`) and they will all be discovered automatically.

---

## Configuration Reference

All configuration lives in `config.toml` at the project root. Every field is a top-level TOML key (no nested tables).

| Field | Type | Purpose | Example |
|-------|------|---------|---------|
| `base_title` | string | Site name, used in `<title>` tags and sidebar | `"My Blog"` |
| `base_tagline` | string | Short tagline, used in RSS feed `<description>` | `"Thoughts on code"` |
| `base_description` | string | Longer description for sidebar and meta tags | `"A developer blog about systems programming"` |
| `base_domain` | string | Bare domain name (no protocol) | `"example.github.io"` |
| `base_url` | string | Full URL prefix including path (no trailing slash) | `"https://example.github.io/mysite"` |
| `base_year` | string | Copyright year displayed in sidebar | `"2026"` |
| `author_name` | string | Author display name | `"Jane Doe"` |
| `author_url` | string | Author profile URL | `"https://example.com/@jane"` |
| `author_email` | string | Author email (used in RSS feed) | `"jane@example.com"` |
| `author_twitter` | string | Twitter/X handle | `"@janedoe"` |
| `opengraph_description` | string | Default OpenGraph description for pages without their own | `"Welcome to my site"` |
| `opengraph_image` | string | Default OpenGraph image URL | `"https://example.com/og.png"` |
| `opengraph_image_alt` | string | Alt text for the OpenGraph image | `"Site logo"` |
| `category_preview` | string | Format string for category meta descriptions. `{}` is replaced with the category name. | `"Category: {}"` |
| `paginate_number` | integer | Number of posts per pagination page | `12` |
| `paginate_format` | string | URL pattern for pagination pages. `{num}` is replaced with the page number. Set to `""` to disable pagination. | `"/p{num}/"` |
| `separator_preview` | string | HTML comment that splits post content into preview and full versions | `"<!--excerpt-->"` |
| `read_more` | string | Link text shown when a post has a preview (excerpt exists) | `"Read more"` |
| `read_original` | string | Link text shown when a post has no excerpt (full content is the preview) | `"Read original"` |

---

## Content Authoring Guide

### Writing Posts

Post files are placed in any qualifying top-level directory (see Post Directory Convention above). Files must have a `.md` or `.html` extension.

A post file uses the ToMH format:

```
+++
<!--break type:header content-->
title = "My First Post"
date = "2026-03-15 10:00:00+08:00"
short = ["my-first-post"]
categories = ["Tutorial", "Python"]
opengraph_description = "A brief introduction to MaLTHA"
opengraph_image = "res/my-image.png"
opengraph_image_alt = "Description of the image"
<!--break type:content format:md content-->
+++

This is the preview portion of the post, visible on the index page.

<!--excerpt-->

This is the full content, only visible on the post's own page.
Everything below the excerpt separator is hidden from the preview.
```

#### Required Header Fields

| Field | Type | Description |
|-------|------|-------------|
| `title` | string | Post title |
| `date` | string | ISO 8601 datetime with timezone offset (e.g., `"2026-03-15 10:00:00+08:00"`) |
| `short` | array of strings | URL slug(s). The first element is the canonical slug. Additional elements create alias URLs. |
| `categories` | array of strings | Category names. Used for URL generation and category pages. |

#### Optional Header Fields

| Field | Type | Description |
|-------|------|-------------|
| `opengraph_description` | string | Per-post OpenGraph description (overrides site default) |
| `opengraph_image` | string | Per-post OpenGraph image URL |
| `opengraph_image_alt` | string | Alt text for the post's OpenGraph image |

#### Content Format

The `<!--break type:content format:X content-->` delimiter accepts two formats:

- `format:md` -- Content is processed through `markdown2` with fenced-code-blocks and tables extras.
- `format:html` -- Content is used as raw HTML without conversion.

Note that even with `format:md`, you can embed raw HTML within the Markdown body. The `+++` delimiters at the start and end of the file are optional legacy markers (they are stripped during parsing).

#### The Excerpt Separator

Place `<!--excerpt-->` (or whatever string `separator_preview` is set to in `config.toml`) within your content to split it into preview and full portions:

- **With separator:** The index page shows only the content above the separator, with a "Read more" link. The post page shows the full content.
- **Without separator:** The index page shows the complete content with a "Read original" link.

The preview content is passed through `oneline()`, which strips newlines and 4-space indentation.

#### Generated URL Aliases

For a post with `short = ["my-post"]`, `categories = ["Cat1", "Cat2"]`, and `date = "2026-03-15 ..."`, the following URL paths are generated:

```
/Cat1/Cat2/2026/03/15/my-post/    # categories + date + short
/Cat1/Cat2/my-post/                # categories + short
/2026/03/15/my-post/               # date + short
/my-post/                          # short only
/post/my-post/                     # /post/ prefix + short
```

The canonical URL is `/{YYYY}/{MM}/{DD}/{short}/` (date + first short slug).

### Writing Standalone Pages

Pages are defined in `page_files/`. Files are processed in sorted order, which affects their display order in the sidebar navigation.

```
<!--break type:header content-->
title = "About"
path = ["/about/"]
page_emoji = "ℹ️"
<!--break type:content format:html content-->
<h1>About This Site</h1>
<p>Welcome to my site.</p>
```

#### Page Header Fields

| Field | Type | Description |
|-------|------|-------------|
| `title` | string | Page title (must be unique across all pages) |
| `path` | array of strings | URL paths for this page. First element is canonical. |
| `page_emoji` | string | Emoji displayed next to the page title in sidebar navigation |
| `skip` | string | Optional. `"list"` removes the page from sidebar nav. `"content"` skips content processing and sidebar `/pages` alias. |
| `frame` | string | Optional. Names a `type:frame` section within the same file to use as page content. |
| `layout` | string | Optional. Names a `type:frame` section to use as `layout_content`, adding an extra format pass. |
| `base` | string | Optional. Names a `type:frame` section to use as the outermost template, bypassing `layout_default` entirely. |

#### Content Resolution Priority

When determining what content to render for a page, the Convertor checks in this order:

1. A `type:content` section in the file (used directly as `page_content`).
2. A `frame` key in the header pointing to a named `type:frame` section.
3. A `layout` key in the header pointing to a named `type:frame` section.

If `layout` is present in the header, that frame is also stored as `layout_content`, and the Generator applies an additional (third) format pass to resolve placeholders from the layout frame.

If `base` is present in the header, the page's own frame content replaces `layout_default` as the outermost HTML wrapper.

#### Page URL Aliases

For `path = ["/about/"]`, the generated URLs are:

```
/about/            # canonical
/pages/about/      # automatic /pages/ prefix alias
```

The `/pages/` alias is suppressed when `skip = "content"`.

For paths ending in a file extension (e.g., `/atom.xml`), the output file is written directly at that path rather than creating a directory with `index.html`.

---

## Template System

### The ToMH File Format

ToMH files use `<!--break ... content-->` delimiters to separate sections. Each delimiter is an HTML comment with space-separated `key:value` attributes followed by the literal word `content-->`.

**Syntax:**

```
<!--break type:TYPE [title:NAME] [format:FORMAT] content-->
...section body...
```

**Section types:**

| Type | Attributes | Description |
|------|------------|-------------|
| `header` | (none) | TOML front matter, parsed with `rtoml` |
| `content` | `format:md` or `format:html` | Page/post body content |
| `include` | `title:NAME` | Reusable fragment, merged into `structure` as `include_NAME` |
| `layout` | `title:NAME` | Layout template, merged into `structure` as `layout_NAME` |
| `format` | `title:NAME` | Format fragment, merged into `structure` as `format_NAME` |
| `frame` | `title:NAME` | File-local template fragment, NOT merged into global `structure` |

Multiple sections of the same type and title within a single file are **concatenated**. This allows you to interleave frame content with format definitions (as seen in `01-index.html` and `02-cat.html` where frame sections wrap around embedded format definitions).

### Include Files (`include_files/`)

Include files define reusable HTML fragments that are injected into layouts. Each include section becomes a key in `self.structure`. For example, `include_files/head.html` defines `include_head`, which is referenced in `layout_default` as `{include_head}`.

Example from `include_files/head.html`:

```html
<!--break type:include title:head content-->
<head>
<title id="title">{title}</title>
<meta name="description" content="{opengraph_description}" />
<link rel="canonical" href="{canonical_url}" />
<link rel="stylesheet" href="{base_url}/main.css?v1770490097" />
</head>
```

### Layout Files (`layout_files/`)

Layout files define the structural templates that wrap content. The primary layout is `layout_default`, which serves as the outermost HTML shell:

```html
<!--break type:layout title:default content-->
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en" lang="en-us">
{include_head}
<body class="theme-tratoh">
<div class="site-wrapper">
{include_sidebar}
<main class="content container">
{layout_content}
</main>
</div>
</body>
</html>
```

The `{layout_content}` placeholder is replaced with a specific sub-layout before rendering begins:

- Posts use `layout_post`
- Pages use `layout_page`
- Categories use `layout_category`
- Pagination uses `layout_pagination`

Layout files can also contain `type:format` sections. For instance, `layout_files/post.html` defines both `layout_post` and several format fragments (`format_categories_in_post`, `format_related_frame`, `format_related_member`).

### Format Fragments

Format fragments are small template strings used during the Convertor stage to render individual list items, category links, sidebar entries, and similar repeated elements. They are defined in any ToMH file with `<!--break type:format title:NAME content-->`.

Key format fragments and where they are defined:

| Fragment | Defined In | Purpose |
|----------|-----------|---------|
| `format_post_container_preview` | `page_files/01-index.html` | Post card with excerpt on index |
| `format_post_container_full` | `page_files/01-index.html` | Post card with full content on index |
| `format_pages_in_sidebar` | `include_files/sidebar.html` | Sidebar navigation link |
| `format_active_pages_in_sidebar` | `include_files/sidebar.html` | Active-state sidebar link |
| `format_categories_in_post` | `layout_files/post.html` | Category tag link on post page |
| `format_related_frame` | `layout_files/post.html` | Related posts container |
| `format_related_member` | `layout_files/post.html` | Individual related post link |
| `format_categories_by_section` | `page_files/02-cat.html` | Category section on categories page |
| `format_member_in_category_section` | `page_files/02-cat.html` | Member link within category section |
| `format_member_in_category_content` | `layout_files/category.html` | Member link on individual category page |
| `format_atom_post` | `page_files/03-atom.xml` | RSS feed item entry |
| `format_pagination_older_active` | `layout_files/pagination.html` | Active "Older" pagination link |
| `format_pagination_older_froze` | `layout_files/pagination.html` | Disabled "Older" pagination span |
| `format_pagination_newer_active` | `layout_files/pagination.html` | Active "Newer" pagination link |
| `format_pagination_newer_froze` | `layout_files/pagination.html` | Disabled "Newer" pagination span |

### Frame Sections

Frame sections (`type:frame`) are local to the file that defines them. They are used exclusively in `page_files/` to provide alternative content or layout structures for specific pages.

A page with `frame = "categories"` in its header will use the `type:frame title:categories` section from that same file as its content. A page with `base = "atom"` will use the corresponding frame as the outermost wrapper, bypassing `layout_default`.

### Multi-Pass Rendering and Brace Escaping

Templates are rendered using Python's `str.format(**dict)`. The Generator applies this **two or three times** per page:

1. **First pass:** Expands top-level layout placeholders (e.g., `{include_head}` becomes the full `<head>` HTML, which itself contains `{title}`, `{base_url}`, etc.)
2. **Second pass:** Expands the placeholders introduced by the first pass.
3. **Third pass (pages with `frame` or `layout` only):** Resolves remaining placeholders from frame content.

After all passes, `{{` is replaced with `{` and `}}` is replaced with `}`.

**Critical constraint:** Because `str.format()` interprets `{...}` as a placeholder, you **cannot** use JavaScript code containing braces (e.g., `function() { ... }`, `if (x) { ... }`) directly in any ToMH template file. The format call will raise a `KeyError` or produce corrupted output.

**Workaround:** Place all JavaScript in standalone `.js` files inside `static_files/` and reference them via `<script src="...">` tags. This is the only safe approach.

To output a literal `{` or `}` in the final HTML, use `{{` and `}}` respectively in your templates.

---

## Setting Up a New General Website

### Step 1: Clone and Install Dependencies

```bash
git clone <your-repo-url> mysite
cd mysite
pip install MaLTHA rtoml markdown2
```

This installs the MaLTHA static site generator along with its dependencies (`rtoml` for TOML parsing, `markdown2` for Markdown-to-HTML conversion).

### Step 2: Configure `config.toml`

Create or edit `config.toml` at the project root:

```toml
"base_title" = "My Website"
"base_tagline" = "A personal website"
"base_description" = "Welcome to my personal website"
"base_domain" = "username.github.io"
"base_url" = "https://username.github.io/mysite"
"base_year" = "2026"
"author_name" = "Your Name"
"author_url" = "https://yoursite.com"
"author_email" = "you@example.com"
"author_twitter" = "@yourhandle"
"opengraph_description" = "Welcome to my personal website"
"opengraph_image" = "https://example.com/og-image.png"
"opengraph_image_alt" = "Site logo"
"category_preview" = "Category: {}"
"paginate_number" = 10
"paginate_format" = "/p{num}/"
"separator_preview" = "<!--excerpt-->"
"read_more" = "Read more"
"read_original" = "Read original"
```

If your site is hosted at the root of a domain (e.g., `https://example.com`), set `base_url` to `"https://example.com"` with no trailing slash. If hosted under a subpath, include it: `"https://example.com/blog"`.

### Step 3: Create the Required Directory Structure

```bash
mkdir -p include_files layout_files page_files static_files posts
```

### Step 4: Create Include Templates

**`include_files/head.html`** -- The `<head>` element:

```html
<!--break type:include title:head content-->
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>{title}</title>
<meta name="description" content="{opengraph_description}" />
<link rel="canonical" href="{canonical_url}" />
<link rel="stylesheet" href="{base_url}/main.css" />
<link rel="alternate" type="application/rss+xml" title="RSS" href="{base_url}/atom.xml" />
</head>
```

**`include_files/sidebar.html`** -- Navigation sidebar:

```html
<!--break type:include title:sidebar content-->
<nav>
<h1><a href="{base_url}/">{base_title}</a></h1>
<p>{base_description}</p>
{page_content_list}
    <!--break type:format title:pages_in_sidebar content-->
    <a href="{page_url}">{page_emoji} {page_title}</a>
    <!--break type:format title:active_pages_in_sidebar content-->
    <a class="active" href="{page_url}">{page_emoji} {page_title}</a>
    <!--break type:include title:sidebar content-->
<span>&copy; {base_year}</span>
</nav>
```

Note how the sidebar file interleaves its own include content with format definitions. The `pages_in_sidebar` and `active_pages_in_sidebar` sections define the normal and active-state sidebar links, while the surrounding `include_sidebar` sections define the sidebar wrapper.

### Step 5: Create Layout Templates

**`layout_files/default.html`** -- Outermost HTML wrapper:

```html
<!--break type:layout title:default content-->
<!DOCTYPE html>
<html lang="en">
{include_head}
<body>
{include_sidebar}
<main>{layout_content}</main>
</body>
</html>
```

**`layout_files/post.html`** -- Post layout:

```html
<!--break type:layout title:post content-->
<article>
    <h1>{post_title}</h1>
    <time>{date_show}</time>
    {post_categories}
    <!--break type:format title:categories_in_post content-->
    <a href="{category_url}">{category_title}</a>
    <!--break type:layout title:post content-->
    {post_content}
</article>
{related_content}
<!--break type:format title:related_frame content-->
<section>
    <h2>Related Posts</h2>
    <ul>{related_posts_list}
        <!--break type:format title:related_member content-->
        <li><a href="{member_url}">{member_short} <small>{member_date}</small></a></li>
        <!--break type:format title:related_frame content-->
    </ul>
</section>
```

**`layout_files/page.html`** -- Page layout:

```html
<!--break type:layout title:page content-->
<div class="page">{page_content}</div>
```

**`layout_files/category.html`** -- Category page layout:

```html
<!--break type:layout title:category content-->
<div>
    <h1>{category_title}</h1>
    <ul>
        {category_content}
        <!--break type:format title:member_in_category_content content-->
        <li><a href="{member_url}">{member_title}</a></li>
        <!--break type:layout title:category content-->
    </ul>
</div>
```

**`layout_files/pagination.html`** -- Pagination layout (required if `paginate_format` is non-empty):

```html
<!--break type:layout title:pagination content-->
<div>{pagination_content_list}</div>
<div class="pagination">
    {pagination_older_button}
    <!--break type:format title:pagination_older_active content-->
    <a href="{}">Older</a>
    <!--break type:format title:pagination_older_froze content-->
    <span>Older</span>
    <!--break type:layout title:pagination content-->
    {pagination_newer_button}
    <!--break type:format title:pagination_newer_active content-->
    <a href="{}">Newer</a>
    <!--break type:format title:pagination_newer_froze content-->
    <span>Newer</span>
    <!--break type:layout title:pagination content-->
</div>
```

### Step 6: Create Your First Page

**`page_files/01-index.html`** -- Home page with post listing:

```html
<!--break type:header content-->
title = "Home"
path = ["/", "/all/"]
layout = "index"
page_emoji = "🏠"
<!--break type:frame title:index content-->
<div class="posts">
    {post_content_list}
    <!--break type:format title:post_container_preview content-->
    <div class="post">
        <h1><a href="{post_url}">{post_title}</a></h1>
        <time>{date_show}</time>
        {content_preview}
        <a href="{post_url}">Read more</a>
    </div>
    <!--break type:format title:post_container_full content-->
    <div class="post">
        <h1><a href="{post_url}">{post_title}</a></h1>
        <time>{date_show}</time>
        {content_full}
    </div>
    <!--break type:frame title:index content-->
</div>
```

The `post_container_preview` and `post_container_full` format definitions are **required** -- they are used globally by the Convertor's `atom()` method to build the post listing.

### Step 7: Write Your First Post

Create `posts/hello-world.md`:

```
+++
<!--break type:header content-->
title = "Hello World"
date = "2026-03-15 12:00:00+00:00"
short = ["hello-world"]
categories = ["General"]
opengraph_description = "My first post"
opengraph_image = ""
opengraph_image_alt = ""
<!--break type:content format:md content-->
+++

Welcome to my new website!

<!--excerpt-->

This is the full content of my first post. Everything above the excerpt
separator is shown as a preview on the index page.
```

### Step 8: Add Static Assets

Place your CSS, JavaScript, images, and other static assets in `static_files/`. This entire directory is copied verbatim to `docs/` at build time.

At minimum, create `static_files/main.css` with your site styles.

### Step 9: Build and Preview

```bash
python3 -m MaLTHA --debug
```

This builds the site with relative URLs (empty `base_url`) into `docs/`. Open `docs/index.html` in a browser to preview.

For a production build with the full `base_url` from `config.toml`:

```bash
python3 -m MaLTHA
```

### Step 10: Deploy

Commit the entire repository (including `docs/` if deploying from the branch, or use the GitHub Actions workflow described below) and push to GitHub.

---

## Build Commands

| Command | Description |
|---------|-------------|
| `python3 -m MaLTHA` | Production build. Uses full `base_url` from `config.toml`. |
| `python3 -m MaLTHA --debug` | Development build. Sets `base_url` to empty string for relative URLs, enabling local file browsing. |
| `python3 -m MaLTHA --skip` | Production build, skipping steps 1-2 (preserves existing `docs/` and skips static file copy). |

**When to use each:**

- **`python3 -m MaLTHA --debug`**: Daily development. Relative URLs work for local file browsing without a web server.
- **`python3 -m MaLTHA`**: CI/CD and production deployment. Used in the GitHub Actions workflow.
- **`--skip` flag**: Useful when only content has changed (no static asset changes). Saves time by preserving the `docs/` directory and skipping the `static_files/` copy.

---

## Known Constraints and Gotchas

### JavaScript Braces in Templates

Python's `str.format()` interprets any `{word}` as a placeholder. JavaScript code like `function() { return x; }` will cause a `KeyError` during rendering. **All JavaScript must be placed in standalone `.js` files in `static_files/`** and loaded via `<script src="...">` tags.

To include a literal `{` or `}` in template output, use `{{` and `}}` respectively.

### Frame Type Not Merged into Global Structure

The `Formator.load()` method only merges `include`, `layout`, and `format` section types into `self.structure`. Sections of type `frame` are intentionally excluded -- they remain file-local and are consumed by the Convertor during page processing. You cannot reference a frame defined in one file from another file's template.

### Pagination Overwrites `index.html`

When pagination is enabled (`paginate_format` is non-empty), the first pagination page writes to `docs/index.html`. This **overwrites** any page file that has `path = ["/"]`. In practice, the index page defined in `page_files/` (e.g., `01-index.html`) is overwritten by the paginated version. If you need the index page to not be paginated, set `paginate_format = ""` in `config.toml`.

### `oneline()` Stripping Behavior

The `Formator.oneline()` method removes all newline characters and all 4-space indent sequences. This is applied to all `format` and `frame` section content (but not to `content` sections). This means:

- Templates that rely on significant whitespace will have it stripped.
- Indentation used for readability in template source files is removed from the output.
- Content in `type:content` sections is NOT affected by `oneline()`.

### `base_url` in Debug vs. Production Mode

In debug mode (`--debug` flag), the `Convertor` sets `base_url` to an empty string, producing relative URL paths. This allows local file browsing without a web server.

In production mode (no `--debug` flag), the full `base_url` from `config.toml` is used, producing absolute URLs suitable for deployment.

### Duplicate Short Slugs

If two posts share the same `short[0]` (canonical slug), the Convertor prints an error but does not halt. The second post will overwrite the first at the same URL path. Ensure all canonical slugs are unique.

### Page Title Uniqueness

Page titles must be unique. If two pages share the same `title`, the Convertor prints an error and skips the duplicate. Pages are keyed by title in the internal dictionary.

### Sort Order

- Posts within a directory are sorted by filename.
- Pages are sorted by filename (hence the numeric prefix convention: `01-index.html`, `02-cat.html`).
- Categories are sorted alphabetically by name.

### Multiple Section Concatenation

When a ToMH file contains multiple `<!--break ... content-->` delimiters with the same `type` and `title`, their bodies are **concatenated**. This is by design -- it enables interleaving content with format definitions (e.g., the index page frame wraps around the post container format definitions).

---

## GitHub Pages Deployment

The repository includes a GitHub Actions workflow at `.github/workflows/pages.yml` that automates deployment.

### How It Works

The workflow triggers on:
- Pushes to the `main` branch
- Manual dispatch from the Actions tab

Steps:
1. Checks out the repository
2. Sets up Python 3.11
3. Installs dependencies from `requirements.txt`
4. Runs `python -m MaLTHA` (production build with full `base_url`)
5. Uploads the `docs/` directory as a GitHub Pages artifact
6. Deploys to GitHub Pages

### Setup Requirements

1. In your repository settings, go to **Pages** and set the source to **GitHub Actions**.
2. Ensure `requirements.txt` lists your dependencies:
   ```
   MaLTHA
   rtoml
   markdown2
   ```
3. The workflow uses concurrency control (`group: "pages"`) to prevent overlapping deployments. In-progress deployments are allowed to complete.

### Permissions

The workflow requires these GitHub token permissions:
- `contents: read` -- to check out the repository
- `pages: write` -- to deploy to GitHub Pages
- `id-token: write` -- for Pages deployment authentication

---

## Template Placeholder Reference

This is a non-exhaustive reference of placeholder names available during rendering, drawn from `config.toml` fields, Convertor-computed values, and structure keys.

### Global (available everywhere via `base_info`)

All `config.toml` fields, plus:

| Placeholder | Source | Description |
|-------------|--------|-------------|
| `{post_content_list}` | Convertor | All posts rendered as HTML cards |
| `{atom_content_list}` | Convertor | All posts rendered as RSS items |
| `{categories_content_list}` | Convertor | All categories rendered as sections |
| `{page_content_list}` | Convertor | Sidebar page navigation links |
| `{current_iso8601}` | Generator | Current build timestamp in ISO 8601 |

### Post Context

| Placeholder | Description |
|-------------|-------------|
| `{post_title}` | Post title |
| `{post_url}` | Canonical post URL |
| `{post_content}` | Full post HTML content |
| `{post_categories}` | Rendered category links |
| `{date_show}` | Human-readable date (e.g., "Sat, Mar 15, 2026") |
| `{date_iso}` | ISO date string from front matter |
| `{date_822}` | RFC 822 date (for RSS) |
| `{date_8601}` | ISO 8601 date |
| `{content_full}` | Full post content HTML |
| `{content_preview}` | Preview excerpt HTML |
| `{more_element}` | "Read more" or "Read original" link |
| `{related_content}` | Rendered related posts section |
| `{opengraph_description}` | Post-specific OG description |
| `{opengraph_image}` | Post-specific OG image |
| `{opengraph_image_alt}` | Post-specific OG image alt text |

### Page Context

| Placeholder | Description |
|-------------|-------------|
| `{page_title}` | Page title |
| `{page_url}` | Canonical page URL |
| `{page_content}` | Page body HTML |
| `{page_emoji}` | Page emoji for sidebar |

### Category Context

| Placeholder | Description |
|-------------|-------------|
| `{category_title}` | Category name |
| `{category_url}` | Category page URL |
| `{category_content}` | Rendered member list |
| `{category_section}` | Rendered member list (section variant) |

### Layout/Structure

| Placeholder | Description |
|-------------|-------------|
| `{include_head}` | Rendered `<head>` element |
| `{include_sidebar}` | Rendered sidebar |
| `{layout_content}` | Inner layout (post/page/category/pagination) |
| `{layout_default}` | Outermost HTML wrapper |
| `{layout_post}` | Post layout template |
| `{layout_page}` | Page layout template |
| `{layout_category}` | Category layout template |
| `{layout_pagination}` | Pagination layout template |

### Pagination Context

| Placeholder | Description |
|-------------|-------------|
| `{pagination_content_list}` | Rendered post cards for current page |
| `{pagination_older_button}` | Older page link or disabled span |
| `{pagination_newer_button}` | Newer page link or disabled span |
