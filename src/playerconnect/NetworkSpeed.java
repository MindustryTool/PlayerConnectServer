package playerconnect;

import arc.math.WindowedMean;
import arc.util.Time;


/** Calculate speed of an arbitrary thing, per seconds. E.g. network speed; in bytes per seconds. */
public class NetworkSpeed {
  protected final WindowedMean upload, download;
  protected long lastUpload, lastDownload, uploadAccum, downloadAccum;
  
  public NetworkSpeed(int windowSec) {
    upload = new WindowedMean(windowSec);
    download = new WindowedMean(windowSec);
  }
  
  public void addDownloadMark() {
    addDownloadMark(1);
  }
  
  public void addDownloadMark(int count) {
    if (Time.timeSinceMillis(lastDownload) >= 1000) {
      lastDownload = Time.millis();
      download.add(downloadAccum);
      downloadAccum = 0;
    }
    downloadAccum += count;
  }
  
  public void addUploadMark() {
    addUploadMark(1);
  }
  
  public void addUploadMark(int count) {
    if (Time.timeSinceMillis(lastUpload) >= 1000) {
      lastUpload = Time.millis();
      upload.add(uploadAccum);
      uploadAccum = 0;
    }
    uploadAccum += count;
  }
  
  /** Number of things per second. E.g. bytes per seconds */
  public float downloadSpeed() {
    return download.mean();
  }
  
  /** Number of things per second. E.g. bytes per seconds */
  public float uploadSpeed() {
    return upload.mean();
  }
}
