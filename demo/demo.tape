# demo.tape
# VHS script for agentic_ssh watch command demo

Set Shell "zsh"
Set FontSize 14
Set Width 1200
Set Height 600
Set Theme "Builtin Dark"
Set Padding 20

# Start command
Type "agentic_ssh watch web-fleet 'uname -a'"
Sleep 500ms
Enter
Sleep 3s

# Navigate Inspection Pane
Type "j"
Sleep 1.5s
Type "j"
Sleep 1.5s

# Scroll log viewport
PageUp
Sleep 1s
PageDown
Sleep 1s

# Exit
Type "q"
Sleep 1s
