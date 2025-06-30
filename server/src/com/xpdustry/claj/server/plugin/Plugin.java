package com.xpdustry.claj.server.plugin;

import com.xpdustry.claj.server.ClajVars;

import arc.files.Fi;
import arc.util.CommandHandler;


public abstract class Plugin {
  /** @return the config file for this plugin, as the file 'plugins/[plugin-name]/config.json'.*/
  public Fi getConfig() {
    return ClajVars.plugins.getConfig(this);
  }

  /** Called after all plugins have been created and commands have been registered.*/
  public void init() {}

  /** Register any commands. */
  public void registerCommands(CommandHandler handler) {}
}
