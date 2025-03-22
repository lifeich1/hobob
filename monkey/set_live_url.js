// @connect      *
// @grant        GM.xmlHttpRequest
// @require      https://cdn.bootcdn.net/ajax/libs/jquery/3.7.1/jquery.min.js

// install hint: https://www.tampermonkey.net/faq.php#Q402

(function() {
  'use strict';

  function dosetliveurl() {
    let url = $('a.living-section__link').href;
    let id = parseInt(window.location.pathname.substr(1)) ;

    GM.xmlHttpRequest({
      method: 'POST',
      url: "http://my-pi:3731/op/setliveurl",
      headers: {
        "Content-Type": "application/json"
      },
      data: JSON.stringify({
        uid: id,
        live: url,
      }),
      onerror: e => { console.error('err setliveurl:', e); },
      onload: resp => {
        console.log('setliveurl result', resp);
      },
    });
  }
  setTimeout(dosetliveurl, 3000);
  setTimeout(dosetliveurl, 15000);
})();
