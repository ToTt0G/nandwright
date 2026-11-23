import java.awt.Component;
import java.awt.Container;
import java.awt.Insets;
import java.awt.Window;
import java.awt.event.KeyEvent;
import java.awt.image.BufferedImage;
import java.io.File;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.concurrent.Callable;
import java.util.concurrent.FutureTask;
import javax.swing.AbstractButton;
import javax.swing.JButton;
import javax.swing.JComboBox;
import javax.swing.JComponent;
import javax.swing.JDialog;
import javax.swing.JFileChooser;
import javax.swing.JLabel;
import javax.swing.JOptionPane;
import javax.swing.JSlider;
import javax.swing.SwingUtilities;

/** Drives the Java VM Emulator through public AWT/Swing APIs. */
public final class KeyDrive {
    private static final Set<String> UI_LABELS = Set.of(
        "Enter Address :", "Text to find :", "Name :", "Animate:", "View:", "Format:",
        "Program", "Stack", "Global Stack", "RAM", "Call Stack", "Static", "Local",
        "Argument", "This", "That", "Temp", "", "Value :"
    );

    private static <T> T onEdt(Callable<T> action) throws Exception {
        var task = new FutureTask<>(action);
        SwingUtilities.invokeAndWait(task);
        return task.get();
    }

    private static void appendComponents(Component component, List<Component> components) {
        components.add(component);
        if (component instanceof Container container) {
            for (Component child : container.getComponents()) {
                appendComponents(child, components);
            }
        }
    }

    private static List<Component> components() {
        var components = new ArrayList<Component>();
        for (Window window : Window.getWindows()) {
            appendComponents(window, components);
        }
        return components;
    }

    private static AbstractButton button(String tooltip) {
        for (Component component : components()) {
            if (component instanceof AbstractButton button && button.isShowing()
                    && tooltip.equals(button.getToolTipText())) {
                return button;
            }
        }
        return null;
    }

    private static Component keyTarget() {
        for (Component component : components()) {
            if (component.getClass().getName().endsWith("KeyboardComponent")) {
                var children = new ArrayList<Component>();
                appendComponents(component, children);
                for (Component child : children) {
                    if (child instanceof JButton && child.getKeyListeners().length > 0) {
                        return child;
                    }
                }
            }
        }
        throw new IllegalStateException("No keyboard input component");
    }

    private static int keyChar(int code) {
        return switch (code) {
            case 128 -> '\n';
            case 129 -> '\b';
            case 140 -> 27;
            default -> code < 128 ? code : KeyEvent.CHAR_UNDEFINED;
        };
    }

    private static int keyCode(int code) {
        return switch (code) {
            case 128 -> KeyEvent.VK_ENTER;
            case 129 -> KeyEvent.VK_BACK_SPACE;
            case 130 -> KeyEvent.VK_LEFT;
            case 131 -> KeyEvent.VK_UP;
            case 132 -> KeyEvent.VK_RIGHT;
            case 133 -> KeyEvent.VK_DOWN;
            case 140 -> KeyEvent.VK_ESCAPE;
            default -> KeyEvent.getExtendedKeyCodeForChar(code);
        };
    }

    private static void send(Component target, int event, int code, int character) throws Exception {
        SwingUtilities.invokeAndWait(() -> {
            target.requestFocusInWindow();
            target.dispatchEvent(new KeyEvent(target, event, System.currentTimeMillis(),
                0, code, (char) character));
        });
    }

    private static String readScreen(int rows) {
        for (Component component : components()) {
            if (component instanceof JComponent screen
                    && screen.getClass().getName().endsWith("ScreenComponent") && screen.isShowing()) {
                Insets border = screen.getInsets();
                var frame = new BufferedImage(screen.getWidth(), screen.getHeight(), BufferedImage.TYPE_INT_RGB);
                var graphics = frame.createGraphics();
                screen.paint(graphics);
                graphics.dispose();
                var words = new StringBuilder();
                for (int y = 0; y < rows; y++) {
                    for (int column = 0; column < 32; column++) {
                        int word = 0;
                        for (int bit = 0; bit < 16; bit++) {
                            int pixel = frame.getRGB(border.left + column * 16 + bit, border.top + y);
                            if (pixel != 0xFFFFFFFF) {
                                word |= 1 << bit;
                            }
                        }
                        words.append(' ').append((short) word);
                    }
                }
                return words.toString();
            }
        }
        throw new IllegalStateException("No visible screen component");
    }

    private static void snapshot(int rows, int phase) throws Exception {
        if (rows > 0) {
            System.out.println("SCREEN " + phase + onEdt(() -> readScreen(rows)));
        }
    }

    private static void typeKeys(String[] args, int observedRows) throws Exception {
        Thread.sleep(1500);
        Component target = onEdt(KeyDrive::keyTarget);
        int phase = 0;
        snapshot(observedRows, phase++);
        for (int i = 3; i < args.length; i++) {
            String event = args[i];
            int pause = event.charAt(0) == 'f' ? 60 : 250;
            int code = Integer.parseInt(event.substring(1));
            int character = keyChar(code);
            send(target, KeyEvent.KEY_PRESSED, keyCode(code), character);
            if (character != KeyEvent.CHAR_UNDEFINED) {
                send(target, KeyEvent.KEY_TYPED, KeyEvent.VK_UNDEFINED, character);
            }
            Thread.sleep(pause);
            snapshot(observedRows, phase++);
            send(target, KeyEvent.KEY_RELEASED, keyCode(code), character);
            Thread.sleep(pause);
            snapshot(observedRows, phase++);
        }
        System.out.println("KEYS DONE");
    }

    private static String statusText() {
        var screenText = new ArrayList<String>();
        for (Component component : components()) {
            if (component instanceof JLabel label && label.isShowing()
                    && !(SwingUtilities.getWindowAncestor(label) instanceof JDialog)) {
                String text = label.getText();
                if (text != null && !UI_LABELS.contains(text.trim())) {
                    screenText.add(text);
                }
            }
        }
        return String.join(" ", screenText).trim();
    }

    private static boolean dismissDialog() {
        for (Window window : Window.getWindows()) {
            if (!(window instanceof JDialog) || !window.isShowing()) {
                continue;
            }
            var children = new ArrayList<Component>();
            appendComponents(window, children);
            String message = children.stream().filter(JOptionPane.class::isInstance)
                .map(JOptionPane.class::cast).map(pane -> String.valueOf(pane.getMessage()))
                .findFirst().orElse("").replace("\n", " | ");
            for (Component child : children) {
                if (child instanceof AbstractButton button
                        && ("Yes".equals(button.getText()) || "OK".equals(button.getText()))) {
                    System.out.println("CLICK " + button.getText() + " DIALOG:" + message);
                    button.doClick();
                    return true;
                }
            }
        }
        return false;
    }

    public static void main(String[] args) throws Exception {
        File script = new File(args[0]).getAbsoluteFile();
        long limitMs = Long.parseLong(args[1]) * 1000;
        int observedRows = Integer.parseInt(args[2]);
        Thread.setDefaultUncaughtExceptionHandler((thread, error) -> {
            System.out.println("JAVA EXCEPTION: " + error);
            System.exit(3);
        });
        VMEmulatorMain.main(new String[0]);
        long start = System.currentTimeMillis();
        AbstractButton load;
        while ((load = onEdt(() -> button("Load Script"))) == null) {
            Thread.sleep(50);
        }
        SwingUtilities.invokeAndWait(() -> {
            for (Component component : components()) {
                if (component instanceof JComboBox<?> box) {
                    for (int i = 0; i < box.getItemCount(); i++) {
                        if ("No animation".equals(String.valueOf(box.getItemAt(i)))) {
                            box.setSelectedIndex(i);
                        }
                    }
                } else if (component instanceof JSlider slider) {
                    slider.setValue(slider.getMaximum());
                }
            }
        });
        SwingUtilities.invokeLater(load::doClick);
        while (!onEdt(() -> {
            for (Component component : components()) {
                if (component instanceof JFileChooser chooser && chooser.isShowing()) {
                    chooser.setSelectedFile(script);
                    chooser.approveSelection();
                    return true;
                }
            }
            return false;
        })) {
            Thread.sleep(50);
        }
        Thread.sleep(300);
        SwingUtilities.invokeAndWait(() -> button("Run").doClick());
        var keys = new Thread(() -> {
            try {
                if (args.length > 3) {
                    typeKeys(args, observedRows);
                }
            } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "keyboard");
        keys.start();

        String last = "";
        while (System.currentTimeMillis() - start < limitMs) {
            Thread.sleep(100);
            if (onEdt(KeyDrive::dismissDialog)) {
                continue;
            }
            String text = onEdt(KeyDrive::statusText);
            if (!text.equals(last)) {
                System.out.println("STATUS: " + text);
                last = text;
            }
            if (!text.isEmpty() && !text.startsWith("Running") && !keys.isAlive()) {
                System.exit(0);
            }
        }
        System.out.println("TIMEOUT");
        System.exit(2);
    }
}
