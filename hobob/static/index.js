console.log("hello")

function cur_filter() {
    return $('select#select-filter-type').val();
}

function cur_order() {
    return cur_filter() + '/' + $('a.nav-link.active').attr('id').replace('-tab-nav', '');
}

function enforce_tab_load() {
    if ($('div.tab-pane.active div.d-flex div.card').length == 0) {
        $('div.tab-pane.active div.d-flex').load('/card/ulist/' + cur_order() + '/0/10');
    }
}

function tabs_reload() {
    $('div#default-list-content').html('default-list');
    $('div#video-list-content').html('video-list');
    $('div#live-list-content').html('live-list');
    enforce_tab_load();
}

function loadmore() {
    if ($('div.tab-pane.active div.d-flex div.card').length == 0) {
        // this situation will be handle by enforce_tab_load
        return;
    }
    console.log('loadmore');
    $('#loading-spinner').show();
    var start = $('div.tab-pane.active div.d-flex div.card').length;
    $.get("/card/ulist/" + cur_order() + '/' + start + '/10', function(data, status) {
        $('#loading-spinner').hide();
        $('div.tab-pane.active div.d-flex[role="list-content"]').append(data);
    })
}

function check_bottom_loadmore() {
    var scrollTop = document.documentElement.scrollTop;
    var scrollHeight = document.documentElement.scrollHeight;
    var clientHeight = document.documentElement.clientHeight;
    if (scrollHeight - scrollTop <= clientHeight) {
        loadmore();
        window.scrollTo(0, Math.max(scrollHeight - clientHeight - 25, 0));
    }
}

var uid_capture = /^(?:(?:https?:\/\/)?space\.bilibili\.com\/)?(\d+)\/?(?:\?.*)?$/;
function submit_follow() {
    var v = $('input[aria-label="Follow"]').val();
    try {
        var id = parseInt(v.match(uid_capture)[1]);
    } catch (e) {
        alert("Error:" + e);
        return;
    }
    do_post_json('/op/follow', {
        enable: true,
        uid: id,
    }, function (d) {
        console.log("succ", d);
        window.scrollTo(0, 0);
        window.location.reload();
    });
    return false;
}

function post_forcesilence(flag) {
    console.log("call post_forcesilence", flag);
    do_post_json('/op/silence', {
        silence: flag,
    });
}

function do_post_json(url, data, success, complete) {
    if (!complete) {
        complete = function (xml, status) {
            console.log("final", status);
        };
    }
    if (!success) {
        success = function (d) {
            console.log("succ", d);
        };
    }
    $.ajax({
        type: "post",
        url: url,
        dataType : "json",
        contentType : "application/json",
        data: JSON.stringify(data),
        complete: complete,
        success: success,
    });
}

function on_force_arefresh() {
    post_forcesilence(false);
}

function on_force_silence() {
    post_forcesilence(true);
}

function on_filter_changed() {
    tabs_reload();
}

function on_check_unfollow(id, name) {
    use_yorn_modal('取消关注确认',
        '确认取消关注用户<span class="text-danger">' + name + '</span>？',
        function() {
            do_post_json('/op/follow', {
                enable: false,
                uid: id,
            }, function() {
                tabs_reload();
            });
        },
    );
}

function on_ui_addto_filter(id) {
    use_yorn_modal('选择列表', 'TODO', function() {
        alert("TODO");
    });
}

function on_drop_from_filter(id, uname) {
    var fid = parseInt(cur_filter());
    if (fid > 0) {
        var fname = $('select#select-filter-type option[value="' + fid + '"]').text();
        use_yorn_modal('从当前列表移除确认',
            '确认从列表<span class="text-danger">' + fname +
            '</span>移除用户<span class="text-danger">' + uname + '</span>？',
            function() {
                do_post_json('/op/mod/filter', {
                    uid: id,
                    fid: fid,
                    priority: -1,
                }, function (d) {
                    tabs_reload();
                });
            }
        );
    }
}

function use_yorn_modal(title, desc, cb) {
    $('div.modal#yes-or-no-modal h5.modal-title').text(title);
    $('div.modal#yes-or-no-modal div.modal-body').html(desc);
    $('div.modal#yes-or-no-modal div.modal-footer button.btn-primary').click(function() {
        cb();
        yorn_modal.hide();
    });
    yorn_modal.show();
}

function on_move2top_filter(id) {
    var fid = parseInt(cur_filter());
    if (fid > 0) {
        do_post_json('/op/mod/filter', {
            uid: id,
            fid: fid,
            priority: Date.now(),
        }, function (d) {
            tabs_reload();
        });
    }
}

function on_try_refresh(id) {
    do_post_json('/op/refresh', {
        uid: id,
    });
}

function handle_ev(ev) {
    var data = JSON.parse(ev.data);
    $("span#status-display").text(data.status_desc);
    if (data.done_refresh) {
        $("span.tag-latest-sync-user").hide();
        $("span#status-last-sync-uid").text("最近刷新uid:" + data.done_refresh);
        $("div#user-card-" + data.done_refresh + " span.tag-latest-sync-user").show();
        var card = $("div#user-card-" + data.done_refresh);
        if (card.length > 0) {
            card.load("/card/one/" + data.done_refresh);
        }
    }
}

var evsrc = null;

var yorn_modal = null;

$(function() {
    yorn_modal = new bootstrap.Modal($('div.modal#yes-or-no-modal')[0]);
    $('#loading-spinner').hide();
    $('a[data-bs-toggle="pill"]').bind('shown.bs.tab', function() {
        enforce_tab_load();
    });
    enforce_tab_load();
    $(window).scroll(function() {
        check_bottom_loadmore();
    });
    $('#btn-follow').click(function() {
        submit_follow();
    });
    $('html, body').animate({ scrollTop: 0}, 500);
    evsrc = new EventSource("ev/engine");
    evsrc.onmessage = function(event) {
        console.log("ev/engine:", event);
        handle_ev(event);
    };
})
