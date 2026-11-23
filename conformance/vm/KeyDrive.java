import java.awt.*;
import java.awt.event.*;
import java.io.File;
import java.util.*;
import javax.swing.*;
// Black-box GUI driver: loads a test script, clicks Run, then injects key events
// into the emulator's keyboard component on a schedule.
// args: script timeoutS events...   events: w<ms> | p<keyCode>,<charCode> | r<keyCode>,<charCode>
//   t<charCode> = press, wait 250 ms, release, wait 250 ms
//   f<charCode> = the same with 60 ms waits (for long lines)
// Used by capture_java_probes.py. Only public AWT/Swing APIs are used: the
// emulator is driven like a user would, as a black box.
public class KeyDrive {
  static java.util.List<Component> all(Component c, java.util.List<Component> out) {
    out.add(c);
    if (c instanceof Container) for (Component k : ((Container)c).getComponents()) all(k, out);
    return out;
  }
  static java.util.List<Component> everything() {
    java.util.List<Component> out = new ArrayList<>();
    for (Window w : Window.getWindows()) all(w, out);
    return out;
  }
  static AbstractButton button(String tip) {
    for (Component c : everything()) if (c instanceof AbstractButton && tip.equals(((JComponent)c).getToolTipText()) && c.isShowing()) return (AbstractButton)c;
    return null;
  }
  static Component keyTarget() {
    for (Component c : everything()) if (c.getClass().getName().endsWith("KeyboardComponent")) {
      for (Component k : all(c, new ArrayList<>())) if (k instanceof JButton && k.getKeyListeners().length > 0) return k;
    }
    return null;
  }
  static void send(Component t, int id, int code, int ch) throws Exception {
    final KeyEvent e = new KeyEvent(t, id, System.currentTimeMillis(), 0, code, ch == 0 ? KeyEvent.CHAR_UNDEFINED : (char)ch);
    SwingUtilities.invokeAndWait(() -> { t.requestFocusInWindow(); t.dispatchEvent(e); });
  }
  // The key char a real keyboard would deliver (0 = none, as for arrows).
  static int keyChar(int ch) {
    if (ch < 128) return ch;
    if (ch == 128) return '\n';
    if (ch == 129) return '\b';
    if (ch == 140) return 27;
    return 0;
  }
  static int vk(int ch) {
    if (ch == 128) return KeyEvent.VK_ENTER;
    if (ch == 129) return KeyEvent.VK_BACK_SPACE;
    if (ch == 130) return KeyEvent.VK_LEFT;
    if (ch == 131) return KeyEvent.VK_UP;
    if (ch == 132) return KeyEvent.VK_RIGHT;
    if (ch == 133) return KeyEvent.VK_DOWN;
    if (ch == 140) return KeyEvent.VK_ESCAPE;
    if (ch >= 'a' && ch <= 'z') return KeyEvent.VK_A + (ch - 'a');
    if (ch >= 'A' && ch <= 'Z') return KeyEvent.VK_A + (ch - 'A');
    if (ch >= '0' && ch <= '9') return KeyEvent.VK_0 + (ch - '0');
    if (ch == '-') return KeyEvent.VK_MINUS;
    if (ch == ' ') return KeyEvent.VK_SPACE;
    if (ch == '+') return KeyEvent.VK_PLUS;
    if (ch == '.') return KeyEvent.VK_PERIOD;
    return KeyEvent.getExtendedKeyCodeForChar(ch);
  }
  static String statusText() {
    StringBuilder sb = new StringBuilder();
    java.util.Set<String> known = new java.util.HashSet<>(java.util.Arrays.asList("Enter Address :", "Text to find :", "Name :", "Animate:", "View:", "Format:", "Program", "Stack", "Global Stack", "RAM", "Call Stack", "Static", "Local", "Argument", "This", "That", "Temp", "", "Value :"));
    for (Component c : everything()) if (c instanceof JLabel && !(SwingUtilities.getWindowAncestor(c) instanceof JDialog)) {
      String t = ((JLabel)c).getText();
      if (t != null && !known.contains(t.trim()) && c.isShowing()) sb.append(t).append(" ");
    }
    return sb.toString().trim();
  }
  public static void main(String[] a) throws Exception {
    String script = new File(a[0]).getAbsolutePath();
    long limitMs = Long.parseLong(a[1]) * 1000;
    final String[] events = Arrays.copyOfRange(a, 2, a.length);
    Thread.setDefaultUncaughtExceptionHandler((t, e) -> { System.out.println("JAVA EXCEPTION: " + e); System.exit(3); });
    VMEmulatorMain.main(new String[0]);
    long t0 = System.currentTimeMillis();
    while (button("Load Script") == null) Thread.sleep(50);
    SwingUtilities.invokeAndWait(() -> {
      for (Component c : everything()) if (c instanceof JComboBox) {
        JComboBox b = (JComboBox)c;
        for (int i=0;i<b.getItemCount();i++) if ("No animation".equals(String.valueOf(b.getItemAt(i)))) b.setSelectedIndex(i);
      }
      for (Component c : everything()) if (c instanceof JSlider) { JSlider s=(JSlider)c; s.setValue(s.getMaximum()); }
    });
    AbstractButton load = button("Load Script");
    SwingUtilities.invokeLater(load::doClick);
    JFileChooser fc = null;
    while (fc == null) { Thread.sleep(50); for (Component c : everything()) if (c instanceof JFileChooser && c.isShowing()) fc = (JFileChooser)c; }
    final JFileChooser f = fc;
    SwingUtilities.invokeAndWait(() -> { f.setSelectedFile(new File(script)); f.approveSelection(); });
    Thread.sleep(300);
    AbstractButton run = button("Run");
    SwingUtilities.invokeAndWait(run::doClick);
    Thread keys = new Thread(() -> {
      try {
        Thread.sleep(1500);
        Component t = keyTarget();
        if (t == null) { System.out.println("NO KEYBOARD COMPONENT"); return; }
        for (String ev : events) {
          char k = ev.charAt(0);
          String rest = ev.substring(1);
          if (k == 'w') { Thread.sleep(Long.parseLong(rest)); continue; }
          if (k == 't' || k == 'f') {
            int ch = Integer.parseInt(rest);
            int pause = k == 't' ? 250 : 60;
            int kc = keyChar(ch);
            send(t, KeyEvent.KEY_PRESSED, vk(ch), kc);
            if (kc != 0) send(t, KeyEvent.KEY_TYPED, KeyEvent.VK_UNDEFINED, kc);
            Thread.sleep(pause);
            send(t, KeyEvent.KEY_RELEASED, vk(ch), kc);
            Thread.sleep(pause);
            continue;
          }
          String[] p = rest.split(",");
          int code = Integer.parseInt(p[0]); int ch = p.length > 1 ? Integer.parseInt(p[1]) : 0;
          send(t, k == 'p' ? KeyEvent.KEY_PRESSED : KeyEvent.KEY_RELEASED, code, ch);
        }
        System.out.println("KEYS DONE");
      } catch (Exception e) { System.out.println("KEY THREAD: " + e); }
    });
    keys.start();
    String last = "";
    while (System.currentTimeMillis() - t0 < limitMs) {
      Thread.sleep(100);
      String txt = statusText();
      for (Window w : Window.getWindows()) if (w instanceof JDialog && w.isShowing()) {
        for (Component c : all(w, new ArrayList<>())) if (c instanceof JOptionPane) txt += " DIALOG:" + String.valueOf(((JOptionPane)c).getMessage());
      }
      boolean clicked = false;
      for (Window w : Window.getWindows()) if (w instanceof JDialog && w.isShowing()) {
        for (Component c : all(w, new ArrayList<>())) if (c instanceof AbstractButton) {
          String bt = ((AbstractButton)c).getText();
          if ("Yes".equals(bt) || "OK".equals(bt)) { AbstractButton yb=(AbstractButton)c; System.out.println("CLICK " + bt + " " + txt.substring(Math.max(0, txt.indexOf("DIALOG:"))).replace("\n"," | ").substring(0, Math.min(200, Math.max(0, txt.length() - Math.max(0, txt.indexOf("DIALOG:")))))); SwingUtilities.invokeAndWait(yb::doClick); clicked = true; break; }
        }
      }
      if (clicked) { Thread.sleep(100); continue; }
      if (!txt.equals(last)) { System.out.println("STATUS: " + txt); last = txt; }
      if (!txt.isEmpty() && !txt.startsWith("Running") && !keys.isAlive()) break;
    }
    if (System.currentTimeMillis() - t0 >= limitMs) System.out.println("TIMEOUT");
    System.exit(0);
  }
}
