console.log("hello")

function cur_order() {
    return $('a.nav-link.active').attr('id').replace('-tab-nav', '');
}

function enforce_tab_load() {
    if ($('div.tab-pane.active div.d-flex div.card').length == 0) {
        $('div.tab-pane.active div.d-flex').load('/card/ulist/' + cur_order() + '/0/10');
    }
}

$(function() {
    $('a[data-bs-toggle="pill"]').bind('shown.bs.tab', function() {
        enforce_tab_load();
    });
    enforce_tab_load();
})
