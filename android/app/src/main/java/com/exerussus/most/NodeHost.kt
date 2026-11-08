package com.exerussus.most

import android.content.Context
import com.exerussus.most.core.MobileException
import com.exerussus.most.core.Started
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.io.File
import com.exerussus.most.core.networkChanged as coreNetworkChanged
import com.exerussus.most.core.start as coreStart
import com.exerussus.most.core.stop as coreStop
import com.exerussus.most.core.validName as coreValidName
import com.exerussus.most.core.version as coreVersion

/** Состояние узла моста внутри приложения. */
sealed interface NodeState {
    data object Stopped : NodeState
    data object Starting : NodeState
    data class Running(val started: Started) : NodeState
    data class Failed(val message: String) : NodeState
}

/**
 * Узел моста (Rust, most-mobile) в этом процессе. Запускает его [NodeService]; экраны только читают [state]
 * и ходят в локальный API через [LocalApi].
 */
object NodeHost {
    private val _state = MutableStateFlow<NodeState>(NodeState.Stopped)
    val state: StateFlow<NodeState> = _state

    fun dataDir(ctx: Context): File = File(ctx.filesDir, "bridge")

    /** Узел уже создавался на этом телефоне (есть bridge.json): имя спрашивать не нужно. */
    fun configured(ctx: Context): Boolean = File(dataDir(ctx), "bridge.json").exists()

    fun validName(name: String): Boolean = coreValidName(name)

    val version: String get() = coreVersion()

    /** Запустить (блокирует до готовности — вызывать не из главного потока). */
    @Synchronized
    fun start(ctx: Context, name: String) {
        val cur = _state.value
        if (cur is NodeState.Running) return
        _state.value = NodeState.Starting
        _state.value = try {
            NodeState.Running(coreStart(dataDir(ctx).absolutePath, name))
        } catch (e: MobileException) {
            NodeState.Failed(e.message ?: "не запустилось")
        } catch (e: Throwable) {
            NodeState.Failed(e.toString())
        }
    }

    @Synchronized
    fun stop() {
        if (_state.value !is NodeState.Running) return
        coreStop()
        _state.value = NodeState.Stopped
    }

    fun networkChanged() {
        if (_state.value is NodeState.Running) coreNetworkChanged()
    }

    fun api(): LocalApi? = (state.value as? NodeState.Running)?.let { LocalApi(it.started.url, it.started.token) }
}
