<?xml version="1.0" encoding="UTF-8"?>
<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform" xmlns:atom="http://www.w3.org/2005/Atom">
<xsl:output method="html" encoding="UTF-8" />
<xsl:template match="/">
<html>
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title><xsl:value-of select="/rss/channel/title" /> — Atom Feed</title>
<style>
body { margin: 0; padding: 2rem; font-family: system-ui, -apple-system, "Segoe UI", sans-serif; background: #0f0f1a; color: #d0d0d8; line-height: 1.6; }
.feed-header { max-width: 760px; margin: 0 auto 2rem; padding-bottom: 1rem; border-bottom: 2px solid #7eb8f7; }
.feed-header h1 { font-size: 1.5rem; margin: 0 0 0.25rem; color: #f0f0f0; font-family: "JetBrains Mono", monospace; }
.feed-header p { margin: 0; color: #6a7a8e; font-size: 0.9rem; }
.feed-notice { max-width: 760px; margin: 0 auto 2rem; padding: 0.9rem 1.2rem; background: #1e1e2e; border-left: 4px solid #7eb8f7; border-radius: 4px; font-size: 0.85rem; color: #8a9aae; }
.feed-items { max-width: 760px; margin: 0 auto; }
.feed-item { margin-bottom: 1.5rem; padding-bottom: 1.5rem; border-bottom: 1px solid #2a2a3e; }
.feed-item:last-child { border-bottom: none; }
.feed-item h2 { font-size: 1.05rem; margin: 0 0 0.25rem; }
.feed-item h2 a { color: #c0d0e0; text-decoration: none; }
.feed-item h2 a:hover { color: #7eb8f7; }
.feed-item .date { font-size: 0.8rem; color: #5a6a7e; margin-bottom: 0.4rem; }
</style>
</head>
<body>
<div class="feed-header">
<h1><xsl:value-of select="/rss/channel/title" /></h1>
<p><xsl:value-of select="/rss/channel/description" /></p>
</div>
<div class="feed-notice">
This is an Atom/RSS feed. Copy the URL into your feed reader to subscribe.
</div>
<div class="feed-items">
<xsl:for-each select="/rss/channel/item">
<div class="feed-item">
<h2><a href="{link}"><xsl:value-of select="title" /></a></h2>
<div class="date"><xsl:value-of select="pubDate" /></div>
</div>
</xsl:for-each>
</div>
</body>
</html>
</xsl:template>
</xsl:stylesheet>
