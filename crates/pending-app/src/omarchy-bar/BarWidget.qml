import QtQuick
import qs.Commons
import qs.Ui

// TasksPending on the bar: a click drops the window down over the screen,
// the next one sends it away with its page as it was. TasksPending writes
// this plugin when Settings turns it on, and keeps it current: edits here
// are overwritten.
BarWidget {
  id: root
  moduleName: "io.github.brunoomariano.tasks-pending"

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    // Font Awesome's tasks mark, which the bar's Nerd Font carries.
    text: ""
    tooltipText: "TasksPending"
    // As an argument list, never through a shell that would read it.
    onPressed: Util.execArgv([root.setting("command", "tasks-pending"), "toggle"])
  }
}
