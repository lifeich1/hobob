
# HOBOB

<a href="https://gitmoji.dev">
  <img src="https://img.shields.io/badge/gitmoji-%20😜%20😍-FFDD67.svg?style=flat-square" alt="Gitmoji">
</a>

*WIP*

An web app for hobo kicked from bibi&lili.


## Design

### Data

```json
{
    "up_info": {
        "<id>": {
            "raw": {
                "videos": "...",
                "info": "..."
            },
            "pick": {
                "live": {"title", "url", "entropy", "isopen"},
                "video": {"title", "url", "ts"},
                "post": {"..."},
                "basic": {"name", "face_url", "id", "ctime", "fid", "ban"}
            }
        }
    },
    "up_by_fid": ["<id>"],
    "up_join_group": { "<group>": {"<uid>":1} },
    "events": [ {
        "type": "live/video/post",
        "live": {"isopen", "..."},
        "video": {"..."},
        "post": {"..."}
    } ],
    "group_info": {
        "<gid>": {"name", "removable"}
    },
    "logs": [{"ts", "level", "msg"}],
    "runtime": {
        "bucket": {"atime", "min_gap", "min_change_gap", "gap"},
        "log_filter": {"..."},
        "event_filter": {"..."},
        "db": {"dump_time", "dump_timeout_min"}
    }
}
```

### Code

```json
{
    "www": {
        "get": ["<any>"],
        "new": {"group", "up"},
        "del": {"group", "up"},
        "cf": {"path", "value"},
        "sse": ""
    }
}
```

## Cross compilation

Use [`cross`](https://github.com/cross-rs/cross).

*HINT*: Enforce `CROSS_CONTAINER_ENGINE=podman` to use podman in linux. (*Docker currently is in trouble*)

## Todo

- [ ] persistence fetch and cache user/video info from remote.
- [ ] webpage: user list in default/video-upload/live-entropy order.
- [ ] rename to hobob, remove deprecated hobob\_app.
- [ ] webpage: backend refresh status display.
- [ ] webpage: server notify data update.
- [ ] backend refresh status control.
- [ ] webpage: upzhu list filter
    - [ ] webpage: display filter with 3 order
    - [ ] webpage: custom filter default order can be modified.
    - [ ] webpage: able to add/remove customized filter (filter 1 is specially unmovable)
- [ ] webpage: show recent stop refresh reason, for banned checking.
- [ ] webpage: feature that get search page of containing words in name.
- [ ] webpage: a video list of user X for temporary utilizing while being banned.
- [ ] webpage: unfollow upzhu.


## License

<a href="http://www.wtfpl.net/"><img
       src="http://www.wtfpl.net/wp-content/uploads/2012/12/wtfpl-badge-4.png"
       width="80" height="15" alt="WTFPL" /></a>

