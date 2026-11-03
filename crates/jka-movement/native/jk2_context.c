/* Remote-client Pmove context, scheduled as pinned MVSDK cg_predict.c
 * CG_PredictPlayerState does for each command it replays. */
#include "jk2_host_internal.h"

/* Verbatim pinned MVSDK code/cgame/cg_predict.c CG_EntityStateToPlayerState
 * (4c7c463638192aead1ff3de8a88cc5de266bf3a2); commented-out upstream blocks omitted. */
static void CG_EntityStateToPlayerState( entityState_t *s, playerState_t *ps ) {
	int		i;

	ps->clientNum = s->number;

	VectorCopy( s->pos.trBase, ps->origin );

	VectorCopy( s->pos.trDelta, ps->velocity );

	VectorCopy( s->apos.trBase, ps->viewangles );

	ps->fd.forceMindtrickTargetIndex = s->trickedentindex;
	ps->fd.forceMindtrickTargetIndex2 = s->trickedentindex2;
	ps->fd.forceMindtrickTargetIndex3 = s->trickedentindex3;
	ps->fd.forceMindtrickTargetIndex4 = s->trickedentindex4;

	ps->saberLockFrame = s->forceFrame;

	ps->electrifyTime = s->emplacedOwner;

	ps->speed = s->speed;

	ps->genericEnemyIndex = s->genericenemyindex;

	ps->activeForcePass = s->activeForcePass;

	ps->movementDir = s->angles2[YAW];
	ps->legsAnim = s->legsAnim;
	ps->torsoAnim = s->torsoAnim;
	ps->clientNum = s->clientNum;

	ps->eFlags = s->eFlags;

	ps->saberInFlight = s->saberInFlight;
	ps->saberEntityNum = s->saberEntityNum;
	ps->saberMove = s->saberMove;
	ps->fd.forcePowersActive = s->forcePowersActive;

	if (s->bolt1)
	{
		ps->duelInProgress = qtrue;
	}
	else
	{
		ps->duelInProgress = qfalse;
	}

	if (s->bolt2)
	{
		ps->dualBlade = qtrue;
	}
	else
	{
		ps->dualBlade = qfalse;
	}

	ps->emplacedIndex = s->otherEntityNum2;

	ps->saberHolstered = s->shouldtarget; //reuse bool in entitystate for players differently
	ps->usingATST = s->teamowner;

	ps->genericEnemyIndex = -1; //no real option for this

	//The client has no knowledge of health levels (except for the client entity)
	if (s->eFlags & EF_DEAD)
	{
		ps->stats[STAT_HEALTH] = 0;
	}
	else
	{
		ps->stats[STAT_HEALTH] = 100;
	}

	ps->weapon = s->weapon;
	ps->groundEntityNum = s->groundEntityNum;

	for ( i = 0 ; i < MAX_POWERUPS ; i++ ) {
		if (s->powerups & (1 << i))
		{
			ps->powerups[i] = 30;
		}
		else
		{
			ps->powerups[i] = 0;
		}
	}

	ps->loopSound = s->loopSound;
	ps->generic1 = s->generic1;
}

/* Without a transitioned snapshot there is no cg_entities context. Saber
 * locks are the case where Pmove's result then depends on missing state. */
static void require_context(const playerState_t *ps, int time) {
    int anim = ps->torsoAnim & ~ANIM_TOGGLEBIT;
    if (ps->saberLockTime > time || ps->saberLockFrame || anim == BOTH_BF2LOCK || anim == BOTH_BF1LOCK
        || anim == BOTH_CWCIRCLELOCK || anim == BOTH_CCWCIRCLELOCK)
        Com_Error(ERR_DROP, "JK2 saber-lock prediction requires remote client state");
}

void jk2_context_pmove(pmove_t *move, entityState_t remote[MAX_CLIENTS], playerState_t send[MAX_CLIENTS],
    const jk2_frame *frame, int context_ready) {
    int i;
    if (!context_ready) require_context(move->ps, move->cmd.serverTime);
    for ( i = 0 ; i < MAX_CLIENTS ; i++ )
    {
        memset(&send[i], 0, sizeof(send[i]));
        CG_EntityStateToPlayerState(&remote[i], &send[i]);
        move->bgClients[i] = &send[i];
    }
    if (context_ready && frame->lock_time > frame->cg_time)
    {
        vec3_t lockDir, lockAng;
        if (!frame->has_lock_origin) Com_Error(ERR_DROP, "JK2 saber-lock opponent %d has no lerped origin", frame->lock_enemy);
        VectorSubtract( frame->lock_origin, frame->snap_origin, lockDir );
        vectoangles(lockDir, lockAng);
        VectorCopy(lockAng, move->ps->viewangles);
    }
    Pmove(move);
    for ( i = 0 ; i < MAX_CLIENTS ; i++ )
    {
        remote[i].torsoAnim = send[i].torsoAnim;
        remote[i].legsAnim = send[i].legsAnim;
        remote[i].forceFrame = send[i].saberLockFrame;
        remote[i].saberMove = send[i].saberMove;
    }
}

/* Pinned MVSDK code/cgame/cg_predict.c CG_TouchItem and
 * CG_TouchTriggerPrediction (4c7c463638192aead1ff3de8a88cc5de266bf3a2).
 * Host substitutions only: cg.predictedPlayerState -> *ps, cg.time -> time,
 * cgs.gametype -> settings->gametype, cg_predictItems -> settings->predict_items,
 * cg_pmove -> *move, cg_triggerEntities -> list, trap_CM_InlineModel(n) -> n
 * (the host trace treats a model absent from the BSP as no contact), and the
 * return value is cg.hyperspace. The commented-out IT_ARMOR/IT_HEALTH/IT_AMMO/
 * IT_HOLDABLE upstream block is omitted. */
static void CG_TouchItem( jk2_centity *cent, playerState_t *ps, const jk2_settings *settings, int time ) {
	gitem_t		*item;

	if ( !settings->predict_items ) {
		return;
	}
	if ( !BG_PlayerTouchesItem( ps, &cent->currentState, time ) ) {
		return;
	}

	if (cent->currentState.eFlags & EF_ITEMPLACEHOLDER)
	{
		return;
	}

	if (cent->currentState.eFlags & EF_NODRAW)
	{
		return;
	}

	// never pick an item up twice in a prediction
	if ( cent->miscTime == time ) {
		return;
	}

	if ( !BG_CanItemBeGrabbed( settings->gametype, &cent->currentState, ps ) ) {
		return;		// can't hold it
	}

	item = &bg_itemlist[ cent->currentState.modelindex ];

	// Special case for flags.  
	// We don't predict touching our own flag
	if( settings->gametype == GT_CTF || settings->gametype == GT_CTY ) {
		if (ps->persistant[PERS_TEAM] == TEAM_RED &&
			item->giTag == PW_REDFLAG)
			return;
		if (ps->persistant[PERS_TEAM] == TEAM_BLUE &&
			item->giTag == PW_BLUEFLAG)
			return;
	}

	if (item->giType == IT_POWERUP &&
		(item->giTag == PW_FORCE_ENLIGHTENED_LIGHT || item->giTag == PW_FORCE_ENLIGHTENED_DARK))
	{
		if (item->giTag == PW_FORCE_ENLIGHTENED_LIGHT)
		{
			if (ps->fd.forceSide != FORCE_LIGHTSIDE)
			{
				return;
			}
		}
		else
		{
			if (ps->fd.forceSide != FORCE_DARKSIDE)
			{
				return;
			}
		}
	}


	// grab it
	BG_AddPredictableEventToPlayerstate( EV_ITEM_PICKUP, cent->currentState.number , ps);

	// remove it from the frame so it won't be drawn
	cent->currentState.eFlags |= EF_NODRAW;
	cent->predicted_nodraw = 1;

	// don't touch it again this prediction
	cent->miscTime = time;

	// if its a weapon, give them some predicted ammo so the autoswitch will work
	if ( item->giType == IT_WEAPON ) {
		ps->stats[ STAT_WEAPONS ] |= 1 << item->giTag;
		if ( !ps->ammo[ item->giTag ] ) {
			ps->ammo[ item->giTag ] = 1;
		}
	}
}

int jk2_touch_triggers(playerState_t *ps, const pmove_t *move, jk2_centity *const *list, int count,
    const jk2_settings *settings, int time) {
	int			i;
	trace_t		trace;
	entityState_t	*ent;
	clipHandle_t cmodel;
	jk2_centity	*cent;
	qboolean	spectator;
	int			hyperspace = 0;

	// dead clients don't activate triggers
	if ( ps->stats[STAT_HEALTH] <= 0 ) {
		return 0;
	}

	spectator = ( ps->pm_type == PM_SPECTATOR );

	if ( ps->pm_type != PM_NORMAL && ps->pm_type != PM_FLOAT && !spectator ) {
		return 0;
	}

	for ( i = 0 ; i < count ; i++ ) {
		cent = list[ i ];
		ent = &cent->currentState;

		if ( ent->eType == ET_ITEM && !spectator ) {
			CG_TouchItem( cent, ps, settings, time );
			continue;
		}

		if ( ent->solid != SOLID_BMODEL ) {
			continue;
		}

		cmodel = ent->modelindex;
		if ( !cmodel ) {
			continue;
		}

		jk2_host_model_trace( &trace, ps->origin, move->mins, move->maxs, cmodel );

		if ( !trace.startsolid ) {
			continue;
		}

		if ( ent->eType == ET_TELEPORT_TRIGGER ) {
			hyperspace = 1;
		} else if ( ent->eType == ET_PUSH_TRIGGER ) {
			BG_TouchJumpPad( ps, ent );
		}
	}

	// if we didn't touch a jump pad this pmove frame
	if ( ps->jumppad_frame != ps->pmove_framecount ) {
		ps->jumppad_frame = 0;
		ps->jumppad_ent = 0;
	}
	return hyperspace;
}
