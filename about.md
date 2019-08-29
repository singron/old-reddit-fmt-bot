# old-reddit-fmt-bot

This is an UNOFFICIAL bot that replies to comments that will not render
correctly in old reddit. Reddit usernames cannot exceed 20 characters or else I
would put "unofficial" in the name.

## How can I easily transform a fenced code block into an indented code block?

### For linux

Copy the contents of your code block to your clipboard, run the below
command, then paste your clipboard to overwrite your code block.

```bash
xclip -out -selection clipboard | sed 's/^/    /' | xclip -in -selection clipboard
```

### For OSX

Copy the contents of your code block to your clipboard, run the below
command in a terminal, then paste your clipboard to overwrite your
code block.

```bash
pbpaste | sed 's/^/    /' | pbcopy
```

If your primary browser is Safari, you can also create a Service for this:

* open Automator
* choose New Document > Service
* check "Output replaces selected text" in the header
* in the right pane, search for "shell", drag "Run Shell Script" to
  the right pane
* select "/bin/sh" as shell
* replace the text (`cat`) by `sed 's/^/    /'`
* save as whatever you want (e.g. "indent" or "code block")

From now on, when in safari (or more generally a "Cocoa" text control)
you can just select a block of text, control-click > Services > Indent
to invoke the workflow on the selected block.

### For Windows

Please contribute similar commands or techniques!

## Why does this bot even exist?

I use old reddit and I found myself manually doing essentially what this bot
does.

## Why doesn't this bot reply with an edited version of my comment?

This is generally against reddit
[bottiquette](https://www.reddit.com/wiki/bottiquette) since it prevents the
user from deleting their comment. Additionally, duplicating large comments can
make the page much noisier.

## Why doesn't reddit just use the new markdown parser in the old and new UI?

That would make sense to me.

## What specifically does this bot detect?

Most differences between the new and old markdown parsers are minor. The only
thing this bot currently detects is fenced code blocks since the rendering is
drastically different and often makes the improperly rendered comment
unreadable.
