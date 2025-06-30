package com.xpdustry.claj.client;

import com.xpdustry.claj.client.dialogs.*;


public class Main extends mindustry.mod.Mod {
  @Override
  public void init() {
    //CLaJServers.refresh(); //will be done when opening the room window
    
    new JoinViaClajDialog();
    new CreateClajRoomDialog();
  }
  
  
  private static String version; // cache the version
  /** @return the mod version, using this class, or {@code null} if mod is not loaded yet. */
  public static String getVersion() {
    if (version == null) {
      mindustry.mod.Mods.LoadedMod mod = mindustry.Vars.mods.getMod(Main.class);
      if (mod != null) version = mod.meta.version;  
    }
    return version;
  }
}
