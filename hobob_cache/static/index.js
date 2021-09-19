console.log("hello")

function cur_order() {
    return $('a.nav-link.active').attr('id').replace('-tab-nav', '');
}

function enforce_tab_load() {
    if ($('div.tab-pane.active div.d-flex div.card').length == 0) {
        $('div.tab-pane.active div.d-flex').load('/card/ulist/' + cur_order() + '/0/10');
    }
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
    $.ajax({
        type: "post",
        url: "/op/follow",
        dataType : "json",
        contentType : "application/json",
        data: JSON.stringify({
            enable: true,
            uid: id,
        }),
        complete: function (xml, status) {
            console.log("final", status);
        },
        success: function (d) {
            console.log("succ", d);
            window.scrollTo(0, 0);
            window.location.reload();
        },
    });
    return false;
}

$(function() {
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
})
