package io.github.artcraftandroid.capcraft.ui.editor.panels

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.artcraftandroid.capcraft.core.ClipSuggestion
import io.github.artcraftandroid.capcraft.core.SpeechListener
import io.github.artcraftandroid.capcraft.core.SpeechModel
import io.github.artcraftandroid.capcraft.core.TimelineView
import io.github.artcraftandroid.capcraft.core.TranscriptLine
import io.github.artcraftandroid.capcraft.core.TranscriptListener
import io.github.artcraftandroid.capcraft.core.cancelSpeech
import io.github.artcraftandroid.capcraft.core.captionModels
import io.github.artcraftandroid.capcraft.core.downloadSpeechModel
import io.github.artcraftandroid.capcraft.core.makeClipProject
import io.github.artcraftandroid.capcraft.engine.Threads
import io.github.artcraftandroid.capcraft.engine.clock
import io.github.artcraftandroid.capcraft.engine.userMessage
import io.github.artcraftandroid.capcraft.ui.Glyph
import io.github.artcraftandroid.capcraft.ui.LucideIcons
import io.github.artcraftandroid.capcraft.ui.Palette
import io.github.artcraftandroid.capcraft.ui.Type
import io.github.artcraftandroid.capcraft.ui.editor.EditorViewModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** How long the clips asked for are: a label and the shortest and longest, in seconds. */
private data class ClipLength(val label: String, val minSeconds: Int, val maxSeconds: Int)

private val LENGTHS = listOf(
    ClipLength("Short", 15, 30),
    ClipLength("Medium", 25, 60),
    ClipLength("Long", 60, 90),
)

/** How many moments are looked for. */
private const val HOW_MANY = 6u

/**
 * The clipper: a long talk cut down to the few moments worth posting. What is said is heard on the
 * phone (as the transcript does it), the best stretches are picked from the words and the sound,
 * and each one kept becomes a project of its own - tall, with a blurred fill and captions - to open
 * and change like any other. Nothing leaves the phone.
 */
@Composable
fun ClipperPanel(vm: EditorViewModel) {
    val scope = rememberCoroutineScope()
    var model by remember { mutableStateOf<SpeechModel?>(null) }
    var job by remember { mutableStateOf<Pair<String, Float>?>(null) }
    var refresh by remember { mutableStateOf(0) }
    // set by a finished download, so transcribing follows on its own
    var then by remember { mutableStateOf(false) }
    var lengthIndex by remember { mutableStateOf(1) }
    var picks by remember { mutableStateOf<List<ClipSuggestion>?>(null) }
    var chosen by remember { mutableStateOf<Set<Int>>(emptySet()) }
    var finding by remember { mutableStateOf(false) }
    // (made so far, to make) while clips are being made
    var making by remember { mutableStateOf<Pair<Int, Int>?>(null) }
    val lines = vm.transcript

    LaunchedEffect(refresh) {
        model = withContext(Threads.thumbnails) {
            runCatching { captionModels().filter { it.id == "tiny" || it.id == "base" } }.getOrDefault(emptyList())
                .let { list -> list.firstOrNull { it.downloaded } ?: list.firstOrNull() }
        }
    }
    DisposableEffect(Unit) { onDispose { if (job != null) runCatching { cancelSpeech() } } }

    fun stageName(stage: String) = when (stage) {
        "download" -> "Downloading"
        "unpack" -> "Unpacking"
        "read" -> "Reading the sound"
        "load" -> "Loading"
        else -> "Listening"
    }

    /** Picks the moments from the transcript, for the length asked. */
    fun find(all: List<TranscriptLine>) {
        val s = vm.session ?: return
        val length = LENGTHS[lengthIndex]
        finding = true
        picks = null
        scope.launch(Threads.commands) {
            val result = runCatching {
                s.suggestClips(all, length.minSeconds * 1_000_000L, length.maxSeconds * 1_000_000L, HOW_MANY)
            }
            launch(Dispatchers.Main) {
                finding = false
                result.onSuccess { found ->
                    picks = found
                    // the best three are kept to start with
                    chosen = found.indices.take(3).toSet()
                }.onFailure { e ->
                    picks = emptyList()
                    vm.message = e.userMessage
                }
            }
        }
    }

    // a transcript, from here or from the Transcript panel, is looked through at once
    LaunchedEffect(lines) {
        if (!lines.isNullOrEmpty() && picks == null && !finding) find(lines)
    }

    val listener = remember {
        object : TranscriptListener {
            override fun onProgress(fraction: Float, stage: String) {
                scope.launch(Dispatchers.Main) { job = stageName(stage) to fraction }
            }

            override fun onDone(lines: List<TranscriptLine>) {
                scope.launch(Dispatchers.Main) {
                    job = null
                    vm.transcript = lines
                    if (lines.isEmpty()) vm.message = "No speech was found"
                }
            }

            override fun onFailed(message: String) {
                scope.launch(Dispatchers.Main) {
                    job = null
                    vm.message = message
                }
            }
        }
    }
    val download = remember {
        object : SpeechListener {
            override fun onProgress(fraction: Float, stage: String) {
                scope.launch(Dispatchers.Main) { job = stageName(stage) to fraction }
            }

            override fun onDone(result: TimelineView?) {
                scope.launch(Dispatchers.Main) {
                    job = null
                    then = true
                    refresh++
                }
            }

            override fun onFailed(message: String) {
                scope.launch(Dispatchers.Main) {
                    job = null
                    vm.message = message
                }
            }
        }
    }

    fun transcribe() {
        val m = model ?: return
        if (!m.downloaded) {
            job = "Downloading" to 0f
            runCatching { downloadSpeechModel(m.id, m.family, download) }.onFailure { job = null; vm.message = it.userMessage }
            return
        }
        val s = vm.session ?: return
        job = "Reading the sound" to 0f
        scope.launch(Threads.commands) {
            runCatching { s.transcript(m.id, listener) }
                .onFailure { e -> launch(Dispatchers.Main) { job = null; vm.message = e.userMessage } }
        }
    }
    LaunchedEffect(model) {
        if (then && model?.downloaded == true) {
            then = false
            transcribe()
        }
    }

    /** Makes a project of each clip kept, one after another, so a failure stops at the one that failed. */
    fun make() {
        val all = vm.transcript ?: return
        val list = picks ?: return
        val todo = chosen.sorted().mapNotNull { list.getOrNull(it) }
        if (todo.isEmpty()) return
        making = 0 to todo.size
        scope.launch {
            // the copies are made from what is on disk
            vm.saveNow()
            val base = vm.timeline?.name?.takeIf { it.isNotBlank() } ?: "Clip"
            var made = 0
            var failure: String? = null
            for ((i, pick) in todo.withIndex()) {
                val result = withContext(Threads.commands) {
                    runCatching { makeClipProject(vm.path, "$base clip ${i + 1}", pick.startUs, pick.endUs, all) }
                }
                result.onSuccess { made++ }.onFailure { failure = it.userMessage }
                making = (i + 1) to todo.size
                if (failure != null) break
            }
            making = null
            vm.message = failure
                ?: if (made == 1) "1 clip made - find it in Projects" else "$made clips made - find them in Projects"
        }
    }

    PanelFrame("Clipper", vm, tall = true, titleOnTop = true, left = {
        if (lines != null && job == null && making == null) {
            Text(
                "Again",
                style = Type.Small.copy(color = Palette.DockText),
                modifier = Modifier.clickable { picks = null; chosen = emptySet(); transcribe() }.padding(8.dp),
            )
        }
    }) {
        val running = job
        val busy = making
        when {
            running != null || busy != null -> Column(Modifier.fillMaxSize().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Spacer(Modifier.height(30.dp))
                val label: String
                val fraction: Float
                if (busy != null) {
                    label = "Making clip ${minOf(busy.first + 1, busy.second)} of ${busy.second}"
                    fraction = busy.first / busy.second.toFloat()
                } else {
                    label = "${running!!.first} ${(running.second * 100).toInt()}%"
                    fraction = running.second
                }
                Text(label, style = Type.BodyStrong)
                Spacer(Modifier.height(12.dp))
                Box(Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)).background(Color(0xFF3A3A3A))) {
                    Box(Modifier.fillMaxWidth(fraction.coerceIn(0f, 1f)).height(4.dp).background(Palette.Accent))
                }
                if (busy == null) {
                    Spacer(Modifier.height(16.dp))
                    Text(
                        "Stop",
                        style = Type.BodyStrong.copy(color = Palette.DockText),
                        modifier = Modifier.clickable { runCatching { cancelSpeech() }; job = null }.padding(8.dp),
                    )
                }
            }
            lines == null -> Column(Modifier.fillMaxSize().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Spacer(Modifier.height(24.dp))
                Glyph(LucideIcons.Scissors, Palette.DockText, size = 34.dp)
                Spacer(Modifier.height(14.dp))
                Text(
                    "Find the best moments of a long video and make a short, tall clip of each, with captions.",
                    style = Type.Body.copy(color = Palette.Text, textAlign = TextAlign.Center),
                )
                Spacer(Modifier.height(22.dp))
                val m = model
                Box(
                    Modifier.fillMaxWidth().height(46.dp).clip(RoundedCornerShape(8.dp)).background(Palette.Accent)
                        .clickable(enabled = m != null) { transcribe() },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        if (m != null && !m.downloaded) "Download and find clips (%.0f MB)".format(m.sizeBytes.toLong() / 1_048_576.0) else "Find clips",
                        style = Type.BodyStrong.copy(fontSize = 16.sp, color = Palette.OnAccent),
                    )
                }
                Spacer(Modifier.height(8.dp))
                Text("Works best on talk. Made on this phone; nothing is uploaded.", style = Type.Tiny.copy(color = Palette.TextSecondary))
            }
            lines.isEmpty() -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("No speech was found", style = Type.Body.copy(color = Palette.TextSecondary))
            }
            else -> Column(Modifier.fillMaxSize()) {
                // how long the clips should be
                Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp)) {
                    LENGTHS.forEachIndexed { i, length ->
                        val on = i == lengthIndex
                        Box(
                            Modifier.padding(end = 8.dp).clip(RoundedCornerShape(16.dp))
                                .background(if (on) Palette.Accent else Color(0xFF3A3A3A))
                                .clickable { if (!on) { lengthIndex = i; find(lines) } }
                                .padding(horizontal = 14.dp, vertical = 6.dp),
                        ) {
                            Text(
                                "${length.label} ${length.minSeconds}-${length.maxSeconds}s",
                                style = Type.Small.copy(color = if (on) Palette.OnAccent else Palette.DockText),
                            )
                        }
                    }
                }
                val found = picks
                when {
                    found == null -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                        Text("Finding the best moments", style = Type.Body.copy(color = Palette.TextSecondary))
                    }
                    found.isEmpty() -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                        Text("No clip could be made from this", style = Type.Body.copy(color = Palette.TextSecondary))
                    }
                    else -> LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 8.dp)) {
                        itemsIndexed(found, key = { i, p -> "$i-${p.startUs}" }) { i, pick ->
                            val kept = i in chosen
                            Row(
                                Modifier.fillMaxWidth().padding(vertical = 3.dp).clip(RoundedCornerShape(8.dp))
                                    .background(if (kept) Color(0xFF2F3A3C) else Color.Transparent)
                                    // a tap on the card plays from its start, to hear it
                                    .clickable { vm.scrub(pick.startUs) }
                                    .padding(start = 10.dp, top = 8.dp, bottom = 8.dp, end = 2.dp),
                                verticalAlignment = Alignment.Top,
                            ) {
                                Column(Modifier.weight(1f)) {
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text("#${i + 1}", style = Type.BodyStrong.copy(color = Palette.Accent))
                                        Spacer(Modifier.width(8.dp))
                                        Text(
                                            "${clock(pick.startUs)} - ${clock(pick.endUs)}  ·  ${(pick.endUs - pick.startUs) / 1_000_000L}s",
                                            style = Type.Small.copy(fontSize = 12.sp),
                                        )
                                        Spacer(Modifier.width(8.dp))
                                        Text("score ${pick.score}", style = Type.Tiny.copy(color = Palette.TextSecondary))
                                    }
                                    Spacer(Modifier.height(4.dp))
                                    Text(
                                        pick.text,
                                        style = Type.Body.copy(fontSize = 14.sp, color = Palette.Text),
                                        maxLines = 3,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                }
                                Box(
                                    Modifier.size(44.dp).clickable { chosen = if (kept) chosen - i else chosen + i },
                                    contentAlignment = Alignment.Center,
                                ) {
                                    Glyph(
                                        if (kept) LucideIcons.SquareCheckBig else LucideIcons.Square,
                                        if (kept) Palette.Accent else Palette.TextSecondary,
                                        size = 22.dp,
                                        description = if (kept) "Leave this clip out" else "Make this clip",
                                    )
                                }
                            }
                        }
                    }
                }
                val count = chosen.size
                Box(
                    Modifier.fillMaxWidth().padding(12.dp).height(46.dp).clip(RoundedCornerShape(8.dp))
                        .background(if (count > 0) Palette.Accent else Color(0xFF3A3A3A))
                        .clickable(enabled = count > 0) { make() },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        if (count == 1) "Make 1 clip" else "Make $count clips",
                        style = Type.BodyStrong.copy(fontSize = 16.sp, color = if (count > 0) Palette.OnAccent else Palette.TextSecondary),
                    )
                }
            }
        }
    }
}
