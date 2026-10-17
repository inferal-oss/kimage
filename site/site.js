"use strict";

const tabs = Array.from(document.querySelectorAll('[role="tab"]'));
function activateTab(tab, focus = false) {
  for (const item of tabs) {
    const selected = item === tab;
    item.setAttribute("aria-selected", String(selected));
    item.tabIndex = selected ? 0 : -1;
    document.getElementById(item.getAttribute("aria-controls")).hidden =
      !selected;
  }
  document.getElementById("file-name").textContent =
    tab.id === "tab-image" ? "image.yaml" : "deployment.yaml";
  if (focus) tab.focus();
}
for (const tab of tabs) {
  tab.addEventListener("click", () => activateTab(tab));
  tab.addEventListener("keydown", (event) => {
    if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      const index =
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? tabs.length - 1
            : (tabs.indexOf(tab) +
                (event.key === "ArrowRight" ? 1 : -1) +
                tabs.length) %
              tabs.length;
      activateTab(tabs[index], true);
    }
  });
}

const copy = document.getElementById("copy-command");
copy.addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(
      "cargo install kimage-control --version 0.5.2 --locked --bin kimage --bin kimage-install",
    );
    copy.textContent = "Copied ✓";
    document.getElementById("copy-status").textContent =
      "Cargo install command copied.";
    setTimeout(() => {
      copy.textContent = "Copy ⧉";
    }, 2200);
  } catch {
    document.getElementById("copy-status").textContent =
      "Could not access the clipboard. Select the command in the terminal to copy it.";
    copy.textContent = "Select to copy";
  }
});
