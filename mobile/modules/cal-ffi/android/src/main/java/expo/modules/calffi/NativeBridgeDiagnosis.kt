package expo.modules.calffi

import android.os.Build
import android.util.Log

/**
 * Why the native bridge failed, captured at the one moment the truth exists.
 *
 * The UniFFI bindings reach `libcal_ffi.so` through a JNA direct mapping set up
 * in `UniffiLib`'s static initialiser. If that initialiser throws — the library
 * missing for the device's ABI, a symbol that is not there, a checksum that does
 * not match the bindings — the JVM reports `ExceptionInInitializerError` **once**,
 * carrying the real cause, and then marks the class unusable. Every later touch
 * gets a bare `NoClassDefFoundError: uniffi.cal_ffi.UniffiLib` with no cause at
 * all.
 *
 * That second error is what users were reporting, and it names nothing: it is
 * consistent with a missing library, a stale set of bindings, a packaging
 * mistake and a JNA problem alike. Two separate diagnoses were made from it and
 * both were wrong, because the evidence had already been discarded by the time
 * anyone saw a message.
 *
 * So the first failure is recorded here, in full, and every subsequent report
 * repeats it instead of the JVM's second-hand version. This matters most where
 * there is no other channel: an installed build on a tester's phone, halfway
 * around the world, with no logcat and no debugger — the message on screen is
 * the whole diagnostic budget, so it has to carry the answer.
 *
 * Deliberately never throws from its own code paths. A diagnostic that fails
 * while diagnosing would replace the real cause with its own.
 */
internal object NativeBridgeDiagnosis {

  private const val TAG = "CalFfi"

  /** The first diagnosis, kept because later attempts see a poorer error. */
  @Volatile
  private var first: String? = null

  /**
   * Describe [error], remember the description, and hand it back.
   *
   * The first call wins: a retry after the initialiser has already failed can
   * only produce `NoClassDefFoundError`, which says less than what was captured
   * the first time round.
   */
  @Synchronized
  fun record(error: Throwable): String {
    first?.let { return it }

    // `take` guards against a self-referential cause chain, which would
    // otherwise hang here rather than report anything.
    val chain = generateSequence(error) { it.cause }.take(16).toList()
    val root = chain.lastOrNull() ?: error

    val text = buildString {
      append("Aperio's native library could not be initialised. ")
      append("Root cause: ")
      append(root.javaClass.name)
      val message = root.message
      if (!message.isNullOrBlank()) {
        append(": ")
        append(shorten(message))
      }
      append(". Load probe: ")
      append(probeLibrary())
      append(". Device ABIs: ")
      append(Build.SUPPORTED_ABIS.joinToString(", "))
      append(". Android SDK ")
      append(Build.VERSION.SDK_INT)
      append(". Chain: ")
      append(chain.joinToString(" <- ") { it.javaClass.simpleName })
    }

    first = text
    Log.e(TAG, text, error)
    return text
  }

  /**
   * Whether [error] is the kind of failure this object is for.
   *
   * The caller must not record anything else. A first attempt that fails for an
   * unrelated reason — a corrupt database, a missing Android context — would
   * otherwise be pinned forever under the headline "native library could not be
   * initialised", and the retry's real reason would never be shown. Reporting
   * the wrong cause confidently is the exact failure this file exists to end.
   */
  fun isBridgeFailure(error: Throwable): Boolean =
    generateSequence(error) { it.cause }.take(16).any {
      it is ExceptionInInitializerError ||
        it is NoClassDefFoundError ||
        it is UnsatisfiedLinkError
    }

  /**
   * Trim install paths down to something a person can read aloud.
   *
   * Android's dlopen message embeds the full APK install path, whose two base64
   * segments are longer than the rest of the message and are announced character
   * by character by a screen reader. The last two segments keep what matters —
   * the ABI directory and the file name.
   */
  private fun shorten(message: String?): String {
    val text = message ?: return "no message"
    return text.split(" ").joinToString(" ") { token ->
      if (token.count { it == '/' } < 2) token else "…/" + token.split("/").takeLast(2).joinToString("/")
    }
  }

  /**
   * Ask the platform loader directly, and report only what it answered.
   *
   * This separates two families that look identical from the outside: the
   * library missing for this device's architecture, versus present and loadable
   * with the fault somewhere above it.
   *
   * It states the fact and draws no conclusion, deliberately. The likeliest
   * failure here is JNA's OWN `libjnidispatch.so` — see the 16 KB alignment
   * note in this module's build.gradle — and in that case `Native.register`
   * dies before it ever reaches `libcal_ffi.so`, which then loads perfectly
   * well on its own. An earlier draft concluded "so the fault is in the
   * bindings" from exactly that, contradicting its own root-cause line one
   * sentence earlier.
   */
  private fun probeLibrary(): String = try {
    System.loadLibrary("cal_ffi")
    "libcal_ffi.so loads on its own for this ABI"
  } catch (probeFailure: Throwable) {
    "libcal_ffi.so does not load either — " +
      probeFailure.javaClass.simpleName + ": " + shorten(probeFailure.message)
  }
}
