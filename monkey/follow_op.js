// @connect      *
// @grant        GM.xmlHttpRequest
// @require      https://cdn.bootcdn.net/ajax/libs/jquery/3.7.1/jquery.min.js

// install hint: https://www.tampermonkey.net/faq.php#Q402
(function() {
  'use strict';

  if ($('.up-name') != null) {
    console.log('q:',$('.up-name')[0]);
    const id = $('.up-name')[0].attributes.href.value.substr(21);
    GM.xmlHttpRequest({
      url: "http://my-pi:3731/get/user/" + id,
      responseType: "json",
      onerror: e => { console.error('get user:', e); },
      onload: resp => {
        const user_info = resp.response;
        console.log('get/user/'+ id, 'resp is:', user_info);
        if (typeof(user_info) != 'string') {
          $('span.follow-btn-inner')[0].hidden=true;
          console.log('已关注');
          return;
        }
        $('span.follow-btn-inner')[0].onclick = (() => {
          console.log('op follow', id);
          GM.xmlHttpRequest({
            method: 'POST',
            url: "http://my-pi:3731/op/follow",
            headers: {
              "Content-Type": "application/json"
            },
            data: JSON.stringify({
              enable: true,
              uid: parseInt(id),
            }),
            onerror: e => { console.error('get user:', e); },
            onload: resp => {
              console.log('follow result', resp);
              $('span.follow-btn-inner')[0].textContent = resp.responseText;
            },
          });
        });
      },
    });

    function regPlaylist() {
      // https://www.bilibili.com/list/30748886?sort_field=pubdate&spm_id_from=333.1387.0.0&oid=113611783543621&bvid=BV15AqjYiEij
      console.log('regPlaylist', $('div.header-login-entry'));
      $('div.header-login-entry')[0].onclick = (() => {
        const bvid = window.location.pathname.split('/')[2];
        console.log('jump list action for', id, bvid);
        window.location.href = "https://www.bilibili.com/list/" + id + "?sort_field=pubdate&spm_id_from=333.1387.0.0&oid=113611783543621&bvid=" + bvid;
      });
    }
    setTimeout(regPlaylist, 2000);
    console.log("monkey follow_op v0.2");
  } else {
    console.error("Not found .up-name html piece");
  }


})();
