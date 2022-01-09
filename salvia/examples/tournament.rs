//! Set up tournament and get updates on current state of contestants at any time.

use salvia::{query, Input, InputAccess, QueryContext, Runtime};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
struct Team(&'static str);

// We will use constants for simplicity.
const MAD_BANANAS: Team = Team("Mad Bananas");
const SHY_STOMPERS: Team = Team("Shy Stompers");
const SMUG_CATS: Team = Team("Smug Cats");

impl Team {
    #[query]
    async fn score(self, cx: &QueryContext) -> u32 {
        let roster: Roster = Tournament.get(cx).await;

        let mut score = 0;
        for &team in roster.iter().filter(|&&team| team != self) {
            {
                let outcome = Match(self, team).get(cx).await;
                score += outcome.into_score();
            }

            {
                let outcome = Match(team, self).get(cx).await;
                score += outcome.flip_side().into_score();
            }
        }

        score
    }

    #[query]
    async fn is_disqualified(self, cx: &QueryContext) -> bool {
        // Tried to cheat points by playing with itself. No do.
        Match(self, self).get(cx).await != MatchResult::Unplayed
    }
}

// Let's pretend every pair of teams can play twice: once on calling side and other time on responding side.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, InputAccess)]
struct Match(Team, Team);

impl Input<MatchResult> for Match {
    fn initial(&self) -> MatchResult {
        MatchResult::Unplayed
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
enum MatchResult {
    Win,
    Tie,
    Loss,
    Unplayed,
}

impl MatchResult {
    fn flip_side(self) -> Self {
        use MatchResult::*;

        match self {
            Win => Loss,
            Tie => Tie,
            Loss => Win,
            Unplayed => Unplayed,
        }
    }

    fn into_score(self) -> u32 {
        match self {
            MatchResult::Win => 3,
            MatchResult::Tie => 1,
            MatchResult::Loss => 0,
            MatchResult::Unplayed => 0,
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, InputAccess)]
struct Tournament;
type Roster = Arc<HashSet<Team>>;

impl Tournament {
    #[query]
    async fn winner(self, cx: &QueryContext) -> Option<Team> {
        use std::collections::HashMap;

        let leaderboard = {
            let roster = self.get(cx).await;
            let mut leaderboard = HashMap::new();
            for &team in roster.iter() {
                if team.is_disqualified(cx).await {
                    continue;
                }

                let score = team.score(cx).await;
                leaderboard.insert(team, score);
            }
            leaderboard
        };

        let (&winner, &score) = leaderboard.iter().max_by_key(|(_, &score)| score)?;

        // There can only be one.
        let tied_count = leaderboard.iter().filter(|(_, &s)| s == score).count();
        if tied_count == 1 {
            Some(winner)
        } else {
            None
        }
    }
}

impl Input<Roster> for Tournament {
    fn initial(&self) -> Roster {
        Default::default()
    }
}

#[tokio::main]
async fn main() {
    use MatchResult::*;

    let rt = Runtime::new().await;

    rt.mutate(|cx| async move {
        // Register our teams:
        let roster = Arc::new(
            [MAD_BANANAS, SHY_STOMPERS, SMUG_CATS]
                .iter()
                .copied()
                .collect(),
        );

        Tournament.set(roster, &cx).await;

        // Play some matches:
        Match(MAD_BANANAS, SHY_STOMPERS).set(Win, &cx).await;
        Match(SHY_STOMPERS, MAD_BANANAS).set(Tie, &cx).await;
        Match(SHY_STOMPERS, SMUG_CATS).set(Loss, &cx).await;
    })
    .await;

    rt.query(|cx| async move {
        // Bananas are winning the tournament:
        assert_eq!(Tournament.winner(&cx).await, Some(MAD_BANANAS));
        assert_eq!(MAD_BANANAS.score(&cx).await, 4);
    })
    .await;

    rt.mutate(|cx| async move {
        // Smug cats manage to get a few points from stompers!
        Match(SMUG_CATS, SHY_STOMPERS).set(Tie, &cx).await;
    })
    .await;

    rt.query(|cx| async move {
        // Now, bananas and cats are tied, so no clear winner:
        assert_eq!(Tournament.winner(&cx).await, None);
        assert_eq!(MAD_BANANAS.score(&cx).await, 4);
        assert_eq!(SMUG_CATS.score(&cx).await, 4);
    })
    .await;

    rt.mutate(|cx| async move {
        // Oh, no! Bananas lost!
        Match(MAD_BANANAS, SMUG_CATS).set(Loss, &cx).await;
    })
    .await;

    rt.query(|cx| async move {
        // Now, cats are in the lead:
        assert_eq!(Tournament.winner(&cx).await, Some(SMUG_CATS));
        assert_eq!(SMUG_CATS.score(&cx).await, 7);
    })
    .await;

    rt.mutate(|cx| async move {
        // Cats are too smug and tried to cheat!
        Match(SMUG_CATS, SMUG_CATS).set(Win, &cx).await;
    })
    .await;

    rt.query(|cx| async move {
        // Cats are disqualified for being too smug, so bananas are back! Well played!
        assert_eq!(Tournament.winner(&cx).await, Some(MAD_BANANAS));
        assert_eq!(MAD_BANANAS.score(&cx).await, 4);
    })
    .await;
}
