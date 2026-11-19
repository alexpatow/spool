# The preview video

`public/videos/spool.mp4` is the video a link preview plays when someone shares the site. It is made here, from code: `spool.html` draws any moment of it with `seek(seconds)`, and `render.ts` steps through it frame by frame in headless Chrome and encodes the frames with ffmpeg.

To change it, edit `spool.html`, then, from `site/`:

```sh
bun video/render.ts --serve       # prints a URL; open it to watch the video play
bun run render:video              # renders over public/videos/spool.mp4
```

It needs Chrome (set `CHROME` to its path if it is somewhere unusual) and ffmpeg. A render takes about two minutes.

The video is read in a phone chat at about 300 pixels wide, a sixth of its size, so keep anything a viewer has to read at 56px or more. Its first and last frames are the same card, so that a player that loops it does not jump.
