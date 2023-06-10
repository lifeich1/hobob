
# HOBOB

<a href="https://gitmoji.dev">
  <img src="https://img.shields.io/badge/gitmoji-%20😜%20😍-FFDD67.svg?style=flat-square" alt="Gitmoji">
</a>

*WIP*

An web app for hobo kicked from bibi&lili.

## Cross compilation

Use [`cross`](https://github.com/cross-rs/cross).

*HINT*: Enforce `CROSS_CONTAINER_ENGINE=podman` to use podman in linux. (*Docker currently is in trouble*)

## Todo

- [x] persistence fetch and cache user/video info from remote.
- [x] webpage: user list in default/video-upload/live-entropy order.
- [x] rename to hobob, remove deprecated hobob\_app.
- [x] webpage: backend refresh status display.
- [x] webpage: server notify data update.
- [x] backend refresh status control.
- [ ] webpage: upzhu list filter
    - [x] webpage: display filter with 3 order
    - [x] webpage: custom filter default order can be modified.
    - [ ] webpage: able to add/remove customized filter (filter 1 is specially unmovable)
- [x] webpage: show recent stop refresh reason, for banned checking.
- [ ] webpage: feature that get search page of containing words in name.
- [ ] webpage: a video list of user X for temporary utilizing while being banned.
- [x] webpage: unfollow upzhu.
- [ ] *publish*: use `include_str!()` to make bin ok to publish.


## License

<a href="http://www.wtfpl.net/"><img
       src="http://www.wtfpl.net/wp-content/uploads/2012/12/wtfpl-badge-4.png"
       width="80" height="15" alt="WTFPL" /></a>

