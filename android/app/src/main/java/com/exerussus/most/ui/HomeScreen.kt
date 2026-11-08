package com.exerussus.most.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.exerussus.most.ApiException
import com.exerussus.most.NodeHost
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONObject

data class PeerRow(val name: String, val title: String, val online: Boolean, val via: String, val error: String)

data class ServerRow(val configured: Boolean, val ok: Boolean, val name: String, val error: String, val linkUp: Boolean)

/** 3.23: свой узел, сервер, соседи. Каналы, чаты и файлы добавятся вкладками. */
@Composable
fun HomeScreen(pendingInvite: String, onInviteUsed: () -> Unit) {
    val api = remember { NodeHost.api() } ?: return
    val scope = rememberCoroutineScope()
    var me by remember { mutableStateOf("") }
    var peers by remember { mutableStateOf(listOf<PeerRow>()) }
    var server by remember { mutableStateOf<ServerRow?>(null) }
    var invite by remember { mutableStateOf("") }
    var note by remember { mutableStateOf("") }

    suspend fun refresh() {
        runCatching {
            val ov = api.get("overview")
            me = ov.optString("node")
            val arr = ov.optJSONArray("peers")
            peers = (0 until (arr?.length() ?: 0)).map { i ->
                val p = arr!!.getJSONObject(i)
                PeerRow(p.optString("name"), p.optString("title"), p.optBoolean("online"), p.optString("via"), p.optString("error"))
            }
            val r = api.get("relay")
            val st = r.optJSONObject("status") ?: JSONObject()
            server = ServerRow(st.optBoolean("configured"), st.optBoolean("ok"), st.optString("name"), st.optString("error"), r.optBoolean("linkUp"))
        }
    }

    suspend fun join(line: String) {
        note = "подключаюсь к серверу…"
        note = try {
            api.post("relay", JSONObject().put("invite", line))
            refresh()
            "сервер подключён"
        } catch (e: ApiException) {
            e.message ?: "не вышло"
        } catch (e: Exception) {
            e.toString()
        }
    }

    LaunchedEffect(Unit) {
        if (pendingInvite.isNotBlank()) {
            join(pendingInvite)
            onInviteUsed()
        }
        while (true) {
            refresh()
            delay(5_000)
        }
    }

    Column(Modifier.fillMaxSize().padding(16.dp)) {
        Text(me.ifEmpty { "…" }, color = MostColors.text, fontSize = 24.sp)
        Text("Мост ${NodeHost.version}", color = MostColors.quiet)
        Spacer(Modifier.height(16.dp))

        // Сервер — одна строка постоянной высоты (интерфейс не прыгает).
        val s = server
        Card {
            Text("Свой сервер", color = MostColors.quiet, fontSize = 13.sp)
            Spacer(Modifier.height(4.dp))
            val (text, color) = when {
                s == null -> "…" to MostColors.quiet
                !s.configured -> "не подключён — вставьте приглашение" to MostColors.warn
                s.ok && s.linkUp -> "на связи: ${s.name}" to MostColors.ok
                s.ok -> "на связи: ${s.name}, туннель поднимается…" to MostColors.warn
                else -> "нет связи: ${s.error}" to MostColors.bad
            }
            Text(text, color = color, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (s != null && !s.configured) {
                Spacer(Modifier.height(8.dp))
                OutlinedTextField(value = invite, onValueChange = { invite = it }, label = { Text("Приглашение MRI-…") }, modifier = Modifier.fillMaxWidth())
                Spacer(Modifier.height(8.dp))
                Button(onClick = { scope.launch { join(invite.trim()) } }, enabled = invite.isNotBlank()) { Text("Подключить") }
            }
            Text(note, color = MostColors.quiet, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.height(20.dp))
        }
        Spacer(Modifier.height(16.dp))
        Text("Соседи", color = MostColors.quiet, fontSize = 13.sp)
        Spacer(Modifier.height(8.dp))
        if (peers.isEmpty()) {
            Text("Пока никого. Подключение соседа через сервер — в 3.23 (приглашение с ПК по QR).", color = MostColors.quiet)
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(peers, key = { it.name }) { p -> PeerItem(p) }
        }
    }
}

@Composable
private fun Card(content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(MostColors.card).padding(14.dp)
    ) { content() }
}

@Composable
private fun PeerItem(p: PeerRow) {
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(MostColors.card).padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(10.dp).clip(CircleShape).background(if (p.online) MostColors.ok else MostColors.quiet))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(p.title.ifEmpty { p.name }, color = MostColors.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val sub = when {
                p.online && p.via.isNotEmpty() -> "на связи · ${p.via}"
                p.online -> "на связи"
                p.error.isNotEmpty() -> p.error
                else -> "не на связи"
            }
            Text(sub, color = MostColors.quiet, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}
