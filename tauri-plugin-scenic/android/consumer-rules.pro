# Tauri finds the plugin class and its @Command methods by reflection,
# and Jackson fills the @InvokeArg classes by field name.
-keep class nb.scenic.plugin.** { *; }
