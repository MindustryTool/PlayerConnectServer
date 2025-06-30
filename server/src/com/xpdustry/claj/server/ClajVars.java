package com.xpdustry.claj.server;

import com.xpdustry.claj.server.plugin.Plugins;
import com.xpdustry.claj.server.util.EventLoop;
import com.xpdustry.claj.server.util.NetworkSpeed;

import arc.files.Fi;


public class ClajVars {
  public static ClajRelay relay;
  public static ClajControl control;
  public static String serverVersion;
  
  public static Fi workingDirectory = new Fi("", arc.Files.FileType.local);
  public static Fi pluginsDirectory = workingDirectory.child("plugins");
  
  public static Plugins plugins;
  public static EventLoop loop;
  public static NetworkSpeed networkSpeed;
}
