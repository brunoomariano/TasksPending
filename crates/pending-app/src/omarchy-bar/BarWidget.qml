import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui

// TasksPending on the bar: a click drops the window down over the screen,
// the next one sends it away with its page as it was. A dot shows while an
// alert rang with the window away. TasksPending writes this plugin when
// Settings turns it on, and keeps it current: edits here are overwritten.
BarWidget {
  id: root
  moduleName: "io.github.brunoomariano.tasks-pending"

  // What the window tells the icon (`tasks-pending-bar.json` right in the
  // session's runtime directory): the dot, and the tooltip naming the
  // alerts. No file, or one that cannot be read, says nothing.
  property bool dot: false
  property string tip: "TasksPending"

  function read(raw) {
    var told = {}
    try {
      told = JSON.parse(raw)
    } catch (e) {
      told = {}
    }
    root.dot = told.dot === true
    root.tip = typeof told.tooltip === "string" && told.tooltip !== "" ? told.tooltip : "TasksPending"
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  FileView {
    path: Quickshell.env("XDG_RUNTIME_DIR") + "/tasks-pending-bar.json"
    watchChanges: true
    printErrors: false
    // `text()` is stale in the change signal itself: reload, then read.
    onFileChanged: reload()
    onLoaded: root.read(text())
    onLoadFailed: root.read("")
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    // Font Awesome's tasks mark, which the bar's Nerd Font carries.
    text: "\uf0ae"
    tooltipText: root.tip
    // As an argument list, never through a shell that would read it.
    onPressed: Util.execArgv([root.setting("command", "tasks-pending"), "toggle"])
  }

  // The dot, at the top right of the mark, in the bar's urgent colour.
  Rectangle {
    visible: root.dot
    width: 6
    height: 6
    radius: 3
    color: root.bar ? root.bar.urgent : Commons.Color.urgent
    anchors.centerIn: button
    anchors.horizontalCenterOffset: Style.bar.iconCanvas / 2
    anchors.verticalCenterOffset: -Style.bar.iconCanvas / 2
  }
}
