using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Threading;
public sealed class ClipBlob { public uint Format; public byte[] Bytes; public ClipBlob(uint format, byte[] bytes) { Format=format; Bytes=bytes; } }
public static class NativeProbe {
 [DllImport("user32.dll")] public static extern bool OpenClipboard(IntPtr owner);
 [DllImport("user32.dll")] public static extern bool CloseClipboard();
 [DllImport("user32.dll")] static extern bool EmptyClipboard();
 [DllImport("user32.dll")] static extern IntPtr GetClipboardData(uint format);
 [DllImport("user32.dll")] static extern IntPtr SetClipboardData(uint format, IntPtr memory);
 [DllImport("user32.dll")] static extern uint EnumClipboardFormats(uint format);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern uint RegisterClipboardFormat(string name);
 [DllImport("kernel32.dll")] static extern IntPtr GlobalAlloc(uint flags, UIntPtr bytes);
 [DllImport("kernel32.dll")] static extern UIntPtr GlobalSize(IntPtr memory);
 [DllImport("kernel32.dll")] static extern IntPtr GlobalLock(IntPtr memory);
 [DllImport("kernel32.dll")] static extern bool GlobalUnlock(IntPtr memory);
 [DllImport("kernel32.dll")] static extern IntPtr GlobalFree(IntPtr memory);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr CreateWindowEx(uint ex,string cls,string title,uint style,int x,int y,int w,int h,IntPtr parent,IntPtr menu,IntPtr instance,IntPtr param);
 [DllImport("user32.dll")] public static extern bool DestroyWindow(IntPtr window);
 [DllImport("user32.dll", EntryPoint="FindWindowW", CharSet=CharSet.Unicode)] static extern IntPtr FindWindowNative(string cls,string title);
 public static IntPtr FindWindow(string cls,string title) { return FindWindowNative(string.IsNullOrEmpty(cls)?null:cls,string.IsNullOrEmpty(title)?null:title); }
 [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window,uint msg,UIntPtr wp,IntPtr lp);
 [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr window,uint msg,IntPtr wp,IntPtr lp);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetMenuString(IntPtr menu,uint position,StringBuilder text,int length,uint flags);
 [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr menu);
 [DllImport("user32.dll")] static extern uint GetMenuState(IntPtr menu,uint item,uint flags);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window,out uint process);
 [DllImport("user32.dll")] public static extern bool RegisterHotKey(IntPtr window,int id,uint modifiers,uint key);
 [DllImport("user32.dll")] public static extern bool UnregisterHotKey(IntPtr window,int id);
 [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,UIntPtr extra);
 public delegate bool EnumProc(IntPtr window,IntPtr param);
 [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc callback,IntPtr param);
 public static IntPtr Owner() { return CreateWindowEx(0x80,"STATIC","TrayKit Verification Clipboard",0x80000000,0,0,0,0,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero,IntPtr.Zero); }
 static void Open(IntPtr owner) { for(int i=0;i<40;i++) { if(OpenClipboard(owner)) return; Thread.Sleep(10); } throw new InvalidOperationException("Cannot open clipboard"); }
 public static List<ClipBlob> Capture(IntPtr owner) {
  Open(owner); try { var blobs=new List<ClipBlob>(); uint format=0;
   while((format=EnumClipboardFormats(format))!=0) {
    if(format==2||format==3||format==9||format==14||(format>=0x300&&format<=0x3ff)) throw new InvalidOperationException("Cannot guarantee restoration of clipboard format "+format+"; aborted before mutation");
    blobs.Add(new ClipBlob(format,ReadOpen(format)));
   } return blobs;
  } finally { CloseClipboard(); }
 }
 static byte[] ReadOpen(uint format) {
  IntPtr handle=GetClipboardData(format); ulong size=GlobalSize(handle).ToUInt64();
  if(handle==IntPtr.Zero||size==0||size>128*1024*1024) throw new InvalidOperationException("Cannot read clipboard format "+format);
  IntPtr ptr=GlobalLock(handle); if(ptr==IntPtr.Zero) throw new InvalidOperationException("Cannot lock clipboard format");
  try { byte[] bytes=new byte[(int)size]; Marshal.Copy(ptr,bytes,0,bytes.Length); return bytes; } finally { GlobalUnlock(handle); }
 }
 public static byte[] Read(uint format,IntPtr owner) { Open(owner); try { return ReadOpen(format); } finally { CloseClipboard(); } }
 public static uint[] Formats(IntPtr owner) { Open(owner); try { var result=new List<uint>(); uint f=0; while((f=EnumClipboardFormats(f))!=0) result.Add(f); return result.ToArray(); } finally { CloseClipboard(); } }
 public static void Put(IntPtr owner,ClipBlob[] blobs) {
  var handles=new List<IntPtr>(); try {
   foreach(var blob in blobs) {
    IntPtr h=GlobalAlloc(2,new UIntPtr((uint)blob.Bytes.Length)); if(h==IntPtr.Zero) throw new OutOfMemoryException(); handles.Add(h);
    IntPtr ptr=GlobalLock(h); if(ptr==IntPtr.Zero) throw new InvalidOperationException("Cannot lock output");
    try { Marshal.Copy(blob.Bytes,0,ptr,blob.Bytes.Length); } finally { GlobalUnlock(h); }
   }
   Open(owner); try {
    if(!EmptyClipboard()) throw new InvalidOperationException("EmptyClipboard failed");
    for(int i=0;i<blobs.Length;i++) { if(SetClipboardData(blobs[i].Format,handles[i])==IntPtr.Zero) throw new InvalidOperationException("SetClipboardData failed"); handles[i]=IntPtr.Zero; }
   } finally { CloseClipboard(); }
  } finally { foreach(var h in handles) if(h!=IntPtr.Zero) GlobalFree(h); }
 }
 public static byte[] FileDrop(string[] paths) {
  byte[] text=Encoding.Unicode.GetBytes(string.Join("\0",paths)+"\0\0"); byte[] data=new byte[20+text.Length];
  Buffer.BlockCopy(BitConverter.GetBytes(20),0,data,0,4); Buffer.BlockCopy(BitConverter.GetBytes(1),0,data,16,4); Buffer.BlockCopy(text,0,data,20,text.Length); return data;
 }
 public static string Hash(byte[] bytes) { using(var sha=SHA256.Create()) return BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-","").ToLowerInvariant(); }
 public static void VerifyRestore(IntPtr owner,List<ClipBlob> baseline) {
  Put(owner,baseline.ToArray()); foreach(var blob in baseline) if(Hash(Read(blob.Format,owner))!=Hash(blob.Bytes)) throw new InvalidOperationException("Clipboard rollback mismatch: "+blob.Format);
  if(Formats(owner).Length!=baseline.Count) throw new InvalidOperationException("Clipboard rollback format count mismatch");
 }
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr window,StringBuilder text,int count);
 [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window,StringBuilder text,int count);
 public static string[] Windows(int process) { var result=new List<string>(); EnumWindows(delegate(IntPtr w,IntPtr p) { uint pid; GetWindowThreadProcessId(w,out pid); if(pid==process) { var t=new StringBuilder(512); var c=new StringBuilder(512); GetWindowText(w,t,512); GetClassName(w,c,512); result.Add(w.ToString("X")+" | "+c+" | "+IsWindowVisible(w)+" | "+t); } return true; },IntPtr.Zero); return result.ToArray(); }
 public static IntPtr TrayKitPopupMenu(uint process) {
  IntPtr result=IntPtr.Zero;
  EnumWindows(delegate(IntPtr w,IntPtr p) {
   uint pid; GetWindowThreadProcessId(w,out pid); if(pid!=process) return true;
   var cls=new StringBuilder(64); GetClassName(w,cls,64); if(cls.ToString()!="#32768") return true;
   IntPtr menu=SendMessage(w,0x1E1,IntPtr.Zero,IntPtr.Zero); int count=GetMenuItemCount(menu); if(count<=0) return true;
   for(uint i=0;i<count;i++) { var text=new StringBuilder(512); GetMenuString(menu,i,text,512,0x400); if(text.ToString()=="退出 TrayKit") { result=w; return false; } }
   return true;
  },IntPtr.Zero);
  return result;
 }
 public static string[] TrayKitMenuItems(uint process) {
  IntPtr popup=TrayKitPopupMenu(process); if(popup==IntPtr.Zero) return new string[0];
  IntPtr menu=SendMessage(popup,0x1E1,IntPtr.Zero,IntPtr.Zero); int count=GetMenuItemCount(menu); var result=new List<string>();
  for(uint i=0;i<count;i++) { var text=new StringBuilder(512); GetMenuString(menu,i,text,512,0x400); result.Add(text.ToString()); }
  return result.ToArray();
 }
 public static int TrayKitMenuItemEnabled(uint process,string label) {
  IntPtr popup=TrayKitPopupMenu(process); if(popup==IntPtr.Zero) return -1;
  IntPtr menu=SendMessage(popup,0x1E1,IntPtr.Zero,IntPtr.Zero); int count=GetMenuItemCount(menu);
  for(uint i=0;i<count;i++) { var text=new StringBuilder(512); GetMenuString(menu,i,text,512,0x400); string value=text.ToString().Split('\t')[0]; if(value==label) return (GetMenuState(menu,i,0x400)&3)==0?1:0; }
  return -1;
 }
 public static void CloseTransientPopups(uint process) {
  EnumWindows(delegate(IntPtr w,IntPtr p) { uint pid; GetWindowThreadProcessId(w,out pid); if(pid==process) { var cls=new StringBuilder(64); GetClassName(w,cls,64); if(cls.ToString()=="#32768") PostMessage(w,0x10,UIntPtr.Zero,IntPtr.Zero); } return true; },IntPtr.Zero);
 }
 public static int VisibleWindows(int process) { int count=0; EnumWindows(delegate(IntPtr w,IntPtr p) { uint pid; GetWindowThreadProcessId(w,out pid); if(pid==process&&IsWindowVisible(w)) count++; return true; },IntPtr.Zero); return count; }
 public static void Shortcut(byte key) { keybd_event(0x11,0,0,UIntPtr.Zero); keybd_event(0x12,0,0,UIntPtr.Zero); Key(key); keybd_event(0x12,0,2,UIntPtr.Zero); keybd_event(0x11,0,2,UIntPtr.Zero); }
 [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window,out Rect rect);
 [DllImport("user32.dll")] static extern bool GetMenuItemRect(IntPtr window,IntPtr menu,uint item,out Rect rect);
 [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] static extern void mouse_event(uint flags,uint dx,uint dy,uint buttons,UIntPtr extra);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 [StructLayout(LayoutKind.Sequential)] struct Point { public int X,Y; }
 [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr window,ref Point point);
 [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr window);
 public static void ClickClient(IntPtr window,int x,int y) { double s=GetDpiForWindow(window)/96.0; var p=new Point { X=(int)(x*s),Y=(int)(y*s) }; ClientToScreen(window,ref p); SetCursorPos(p.X,p.Y); mouse_event(2,0,0,0,UIntPtr.Zero); mouse_event(4,0,0,0,UIntPtr.Zero); }
 [StructLayout(LayoutKind.Sequential)] struct KeyboardInput { public ushort Vk,Scan; public uint Flags,Time; public UIntPtr Extra; }
 [StructLayout(LayoutKind.Explicit,Size=32)] struct InputValue { [FieldOffset(0)] public KeyboardInput Keyboard; }
 [StructLayout(LayoutKind.Sequential)] struct Input { public uint Type; public InputValue Value; }
 [DllImport("user32.dll",SetLastError=true)] static extern uint SendInput(uint count,Input[] inputs,int size);
 static Input KeyInput(ushort vk,ushort scan,uint flags) { return new Input { Type=1,Value=new InputValue { Keyboard=new KeyboardInput { Vk=vk,Scan=scan,Flags=flags } } }; }
 public static void CaptureShortcut(IntPtr window,ushort key,bool shift) {
  if(GetForegroundWindow()!=window) throw new InvalidOperationException("Hotkey capture window is not foreground");
  var inputs=new List<Input> { KeyInput(0x11,0,0),KeyInput(0x12,0,0) };
  if(shift) inputs.Add(KeyInput(0x10,0,0));
  inputs.Add(KeyInput(key,0,0)); inputs.Add(KeyInput(key,0,2));
  if(shift) inputs.Add(KeyInput(0x10,0,2));
  inputs.Add(KeyInput(0x12,0,2)); inputs.Add(KeyInput(0x11,0,2));
  Input[] values=inputs.ToArray(); if(SendInput((uint)values.Length,values,Marshal.SizeOf(typeof(Input)))!=(uint)values.Length) throw new InvalidOperationException("SendInput failed");
  Thread.Sleep(300); // SendInput guarantees queue order, not that the target UI already consumed it.
 }
 [StructLayout(LayoutKind.Sequential)] struct NotifyIdentifier { public uint Size; public IntPtr Window; public uint Id; public Guid Guid; }
 [DllImport("shell32.dll")] static extern int Shell_NotifyIconGetRect(ref NotifyIdentifier identifier,out Rect rect);
 public static Rect TrayRectangle(IntPtr window) { var id=new NotifyIdentifier { Size=(uint)Marshal.SizeOf(typeof(NotifyIdentifier)),Window=window,Id=1 }; Rect rect; int hr=Shell_NotifyIconGetRect(ref id,out rect); if(hr!=0) throw new InvalidOperationException("Tray icon is not registered in Shell: "+hr.ToString("X")); return rect; }
 [DllImport("user32.dll")] public static extern uint GetClipboardSequenceNumber();
 [DllImport("user32.dll")] public static extern IntPtr GetClipboardOwner();
 public static uint ClipboardOwnerProcess() { uint pid; GetWindowThreadProcessId(GetClipboardOwner(),out pid); return pid; }
 public static void ScrollClient(IntPtr window,int x,int y,int delta) { double s=GetDpiForWindow(window)/96.0; var p=new Point { X=(int)(x*s),Y=(int)(y*s) }; ClientToScreen(window,ref p); SetCursorPos(p.X,p.Y); mouse_event(0x800,0,0,unchecked((uint)delta),UIntPtr.Zero); }
 public static void ClickMenu(uint process,uint item) { IntPtr popup=TrayKitPopupMenu(process); IntPtr menu=SendMessage(popup,0x1E1,IntPtr.Zero,IntPtr.Zero); Thread.Sleep(100); Rect r; if(!GetMenuItemRect(IntPtr.Zero,menu,item,out r)) throw new InvalidOperationException("Cannot find menu item rectangle"); SetCursorPos((r.Left+r.Right)/2,(r.Top+r.Bottom)/2); mouse_event(2,0,0,0,UIntPtr.Zero); mouse_event(4,0,0,0,UIntPtr.Zero); }
 public static void Key(byte key) { keybd_event(key,0,0,UIntPtr.Zero); keybd_event(key,0,2,UIntPtr.Zero); }
}
