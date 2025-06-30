package com.xpdustry.claj.server;

import com.xpdustry.claj.server.util.JsonSettings;

import arc.struct.Seq;


public class ClajConfig {
  public static final String fileName = "config.json";
  
  protected static JsonSettings settings;
  
  /** Debug log level enabled or not */
  public static boolean debug = false;
  /** Limit for packet count sent within 3 sec that will lead to a disconnect. Note: only for clients, not hosts. */
  public static int spamLimit = 300;
  /** Warn a client that trying to create a room, that it's CLaJ version is deprecated. */
  public static boolean warnDeprecated = true;
  /** Warn all clients when the server is closing */
  public static boolean warnClosing = true;
  /** Simple ip blacklist */
  public static Seq<String> blacklist = new Seq<>();


  @SuppressWarnings("unchecked")
  public static void load() {
    // Load file
    if (settings == null) 
      settings = new JsonSettings(new arc.files.Fi(fileName, arc.Files.FileType.local));
    settings.load();
    
    // Load values
    debug = settings.getBool("debug", debug);
    spamLimit = settings.getInt("spam-limit", spamLimit);
    warnDeprecated = settings.getBool("warn-deprecated", warnDeprecated);
    warnClosing = settings.getBool("warn-closing", warnClosing);
    blacklist = settings.get("blacklist", Seq.class, String.class, blacklist);
    
    // Will create the file of not existing yet, 
    // but also to avoid a NoClassDefFoundError when stopping the server.
    save(); 
  }

  public static void save() {
    settings.put("debug", debug);
    settings.put("spam-limit", spamLimit);
    settings.put("warn-deprecated", warnDeprecated);
    settings.put("warn-closing", warnClosing);
    settings.put("blacklist", String.class, blacklist);
    
    // Save file
    settings.save();
  }
}
