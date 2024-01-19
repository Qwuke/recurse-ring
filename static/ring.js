function mod(n, m) {
  return ((n % m) + m) % m;
}

function replace_hrefs(data) {
  var home = document.getElementById("rc-ring-home");
  var prev = document.getElementById("rc-ring-prev");
  var next = document.getElementById("rc-ring-next");

  var currentUuid = home.getAttribute("data-rc-uuid");
  var currentIndex = data.findIndex((site) => site.website_uuid === currentUuid);

  var prevSite = data[mod((currentIndex - 1), data.length)];
  var nextSite = data[mod((currentIndex + 1), data.length)];
  
  next.setAttribute("href", nextSite.url);
  prev.setAttribute("href", prevSite.url);
}

window.onload = (_event) => {
  var xhr = new XMLHttpRequest();
  xhr.open('GET', 'https://raw.githack.com/Qwuke/recurse-ring/main/sites.json', true);
  
  xhr.onload = function() {
    if (xhr.status >= 200 && xhr.status < 400) {
      replace_hrefs(JSON.parse(xhr.responseText));
    } else {
      var xhr_backup = new XMLHttpRequest();
      xhr_backup.open('GET', 'https://ring.recurse.com/sites.json', true);
      xhr_backup.onload = function() {
        if (xhr_backup.status >= 200 && xhr_backup.status < 400) {
          replace_hrefs(JSON.parse(xhr_backup.responseText));
        } else {  
          console.log("There was an error embedding the static blog URLs");
        }
      };
      xhr_backup.send();
    }
  };
  xhr.send();
};
