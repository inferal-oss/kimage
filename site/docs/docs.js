"use strict";

const contents = document.querySelector(".docs-toc");
const narrow = window.matchMedia("(max-width: 800px)");
function setContentsLayout() {
  contents.open = !narrow.matches;
}
narrow.addEventListener("change", setContentsLayout);
setContentsLayout();
