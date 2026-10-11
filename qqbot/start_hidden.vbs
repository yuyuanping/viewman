' Window-free entry point: takes one argument (start.bat path) and runs it with SW_HIDE (=0).
' viewman calls: wscript start_hidden.vbs "<bot_root>\start.bat"
If WScript.Arguments.Count > 0 Then
    bat = WScript.Arguments(0)
Else
    bat = "D:\opencword\viewman\qqbot\start.bat"
End If
CreateObject("Wscript.Shell").Run Chr(34) & bat & Chr(34), 0, False
