/* Minimal reproducer for a use-after-free in GtkIMContextWayland (GTK 4.20.4
 * and current main).
 *
 * Build & run on a Wayland session whose compositor implements
 * zwp_text_input_v3 (e.g. GNOME Shell / Mutter):
 *   cc gtk_im_uaf_upstream.c -o r $(pkg-config --cflags --libs gtk4)
 *   ./r
 *
 * Or headless:
 *   mutter --headless --wayland --no-x11 --wayland-display wl-r \
 *          --virtual-monitor 1280x720 &
 *   WAYLAND_DISPLAY=wl-r GDK_BACKEND=wayland ./r
 *
 * Expected: SIGSEGV in notify_im_change -> gtk_im_context_wayland_get_global
 * -> gtk_widget_get_display(freed), preceded by three GTK_IS_WIDGET /
 * G_IS_OBJECT / GDK_IS_WAYLAND_DISPLAY criticals.
 *
 * Cause: on the first focus_in, GtkIMContextWayland creates its per-display
 * global, sends wl_registry.get_registry (reply dispatched only on a later
 * loop turn, so global->text_input is still NULL) and sets
 * global->current = this context.  If the widget is unrealized before that
 * reply arrives, GtkText calls gtk_im_context_set_client_widget(ctx, NULL),
 * the multicontext drops and finalizes the wayland delegate, but
 * gtk_im_context_wayland_focus_out() returns early while text_input == NULL
 * and never clears global->current.  The subsequent zwp_text_input_v3.enter
 * event then calls notify_im_change() on the freed context.
 */
#include <gtk/gtk.h>

static void
activate (GtkApplication *app, gpointer user_data)
{
  GtkWidget *window = gtk_application_window_new (app);
  GtkWidget *text = gtk_text_new ();

  gtk_window_set_default_size (GTK_WINDOW (window), 400, 100);
  gtk_window_set_child (GTK_WINDOW (window), text);
  gtk_window_present (GTK_WINDOW (window));

  /* focus_in on the wayland IM context: global created, registry request
   * queued (reply not yet read), global->current = delegate. */
  gtk_widget_grab_focus (text);

  /* Unrealize the focused widget in the same turn, before the registry
   * reply: delegate finalized, but global->current still points at it. */
  gtk_window_set_child (GTK_WINDOW (window),
                        gtk_label_new ("focused widget torn down"));
}

int
main (int argc, char **argv)
{
  GtkApplication *app = gtk_application_new ("org.example.GtkImUaf",
                                             G_APPLICATION_DEFAULT_FLAGS);
  g_signal_connect (app, "activate", G_CALLBACK (activate), NULL);
  int status = g_application_run (G_APPLICATION (app), argc, argv);
  g_object_unref (app);
  return status;
}
